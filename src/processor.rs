use solana_program::{
	account_info::{next_account_info, AccountInfo},
	entrypoint::ProgramResult,
	program_error::ProgramError,
	msg,
	pubkey::Pubkey,
	program_pack::{Pack, IsInitialized},
	sysvar::{rent::Rent, Sysvar},
	program::{invoke, invoke_signed},
};

use spl_token::state::Account as TokenAccount;

use crate::{instruction::EscrowInstruction, error::EscrowError, state::Escrow};

pub struct Processor;
impl Processor {
	pub fn process(program_id: &Pubkey, accounts: &[AccountInfo], instruction_data: &[u8]) -> ProgramResult {
		let instruction = EscrowInstruction::unpack(instruction_data)?;

		match instruction {
			EscrowInstruction::InitEscrow { amount } => {
				msg!("Instruction: InitEscrow");
				Self::process_init_escrow(accounts, amount, program_id)
			},
			EscrowInstruction::Exchange { amount } => {
				msg!("Instruction: Exchange");
				Self::process_exchange(accounts, amount, program_id)
			},
			EscrowInstruction::Cancel { } => {
				msg!("Instruction: Cancel");
				Self::process_cancel(accounts, program_id)
			}
		}
	}

	fn process_init_escrow(account_infos: &[AccountInfo], amount: u64, program_id: &Pubkey) -> ProgramResult {
		let account_info_iter = &mut account_infos.iter();
		let initializer_account_info = next_account_info(account_info_iter)?;

		if !initializer_account_info.is_signer {
			return Err(ProgramError::MissingRequiredSignature);
		}

		// This program must be owned by the Solana Token Program
		let temp_token_account_info = next_account_info(account_info_iter)?;

		// This one too, but we actually check it here. Why don't we check previously?
		let token_to_receive_account_info = next_account_info(account_info_iter)?;
		if *token_to_receive_account_info.owner != spl_token::id() {
			return Err(ProgramError::IncorrectProgramId);
		}
		// Also need to check if token_to_receive account is not a token mint account. If this unpack fails, then we error out
		// If it's a spl_token::state::Mint, this unpack will fail
		TokenAccount::unpack(&token_to_receive_account_info.try_borrow_data()?)?;

		let escrow_account_info = next_account_info(account_info_iter)?;
		let rent = &Rent::from_account_info(next_account_info(account_info_iter)?)?;

		if !rent.is_exempt(escrow_account_info.lamports(), escrow_account_info.data_len()) {
			return Err(EscrowError::NotRentExempt.into());
		}

		// unpack_unchecked comes from default functions from trait in program_pack 
		// https://docs.rs/solana-program/latest/src/solana_program/program_pack.rs.html#29-39
		// try_borrow_data fetches the "data" field from the AccountInfo struct
		let mut escrow_info = Escrow::unpack_unchecked(&escrow_account_info.try_borrow_data()?)?;
		if escrow_info.is_initialized() {
			return Err(ProgramError::AccountAlreadyInitialized);
		}

		// Now that we know escrow struct is uninitialized, let's initialize 
		escrow_info.is_initialized = true;
		escrow_info.initializer_pubkey = *initializer_account_info.key;
		escrow_info.temp_token_account_pubkey = *temp_token_account_info.key;
		escrow_info.initializer_token_to_receive_account_pubkey = *token_to_receive_account_info.key;
		escrow_info.expected_amount = amount;

		Escrow::pack(escrow_info, &mut escrow_account_info.try_borrow_mut_data()?)?;

		// Program Derived Address
		// Why do we seed with address of byte array "escrow"? A: It's just good convention. Also makes it easy to refer later on.
		// PDA are NOT on the ed25519 curve, meaning not possible to collide with Solana key pairs
		let (pda, _bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);

		let token_program_account_info = next_account_info(account_info_iter)?;
		let owner_change_ix = spl_token::instruction::set_authority(
			token_program_account_info.key,
			temp_token_account_info.key, // set_authority will fail if temp_token_account is not owned by Token program
			Some(&pda),
			spl_token::instruction::AuthorityType::AccountOwner,
			initializer_account_info.key,
			&[&initializer_account_info.key],
		)?;

		msg!("Calling the token program to transfer token account ownership...");
		invoke( // Calls the token program FROM our escrow program
			&owner_change_ix,
			&[
				temp_token_account_info.clone(),
				initializer_account_info.clone(),
				token_program_account_info.clone(),
			]	
		)?;

		Ok(())
	}

	fn process_exchange(account_infos: &[AccountInfo], amount_expected_by_taker: u64, program_id: &Pubkey) -> ProgramResult {
		let account_info_iter = &mut account_infos.iter();
		let taker_account_info = next_account_info(account_info_iter)?;

		if !taker_account_info.is_signer {
			return Err(ProgramError::MissingRequiredSignature);
		}	

		let takers_sending_account_info = next_account_info(account_info_iter)?;
		let takers_token_to_receive_account_info = next_account_info(account_info_iter)?;

		let pda_temp_token_account_info = next_account_info(account_info_iter)?;
		let pda_temp_token_account = TokenAccount::unpack(&pda_temp_token_account_info.try_borrow_data()?)?;
		let (pda, bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);

		// Amount validation, prevent frontrunning
		if amount_expected_by_taker != pda_temp_token_account.amount {
			return Err(EscrowError::ExpectedAmountMismatch.into()); // TODO why do we need .into?
		}

		let initializers_main_account_info = next_account_info(account_info_iter)?;
		let initializers_token_to_receive_account_info = next_account_info(account_info_iter)?;
		let escrow_account_info = next_account_info(account_info_iter)?;

		let escrow = Escrow::unpack(&escrow_account_info.try_borrow_data()?)?;

		// Validate Escrow matches instruction 
		if escrow.temp_token_account_pubkey != *pda_temp_token_account_info.key {
			return Err(ProgramError::InvalidAccountData);
		}
		if escrow.initializer_pubkey != *initializers_main_account_info.key {
			return Err(ProgramError::InvalidAccountData);
		}
		if escrow.initializer_token_to_receive_account_pubkey != *initializers_token_to_receive_account_info.key {
			return Err(ProgramError::InvalidAccountData);
		}

		let token_program_account_info = next_account_info(account_info_iter)?;

		let transfer_to_initializer_ix = spl_token::instruction::transfer(  // TODO do the instructions in spl_token::instruction encompass all possible instructions in solana??
			token_program_account_info.key, // token program ID
        	takers_sending_account_info.key, // source pubkey
        	initializers_token_to_receive_account_info.key, // destination pubkey
        	taker_account_info.key,  // authority pubkey
        	&[&taker_account_info.key],  // signer pubkeys
        	escrow.expected_amount,
		)?;
		msg!("Calling the token program to transfer tokens to the escrow's initializer...");
		invoke(
			&transfer_to_initializer_ix,
			&[
				takers_sending_account_info.clone(),
				initializers_token_to_receive_account_info.clone(),
				taker_account_info.clone(),
				token_program_account_info.clone()
			]
		)?;

		let pda_account_info = next_account_info(account_info_iter)?;
		let transfer_to_taker_ix = spl_token::instruction::transfer(
			token_program_account_info.key,
			pda_temp_token_account_info.key,
			takers_token_to_receive_account_info.key,
			&pda,
			&[&pda],
			pda_temp_token_account.amount,
		)?;
		msg!("Calling the token program to transfer tokens to the taker...");
		invoke_signed(
		    &transfer_to_taker_ix,
		    &[
		        pda_temp_token_account_info.clone(),
		        takers_token_to_receive_account_info.clone(),
		        pda_account_info.clone(),
		        token_program_account_info.clone(),
		    ],
		    // This parameter is for authority. In this case, the authority is the PDA. BUT instead of passing in the key for PDA itself, we pass in the seeds (&[b"escrow"] and bump_seed), so that we can recalculate the PDA. If the recalculation and the given PDA keys dont' match, then this instruction fails with AuthenticationError
		    &[&[&b"escrow"[..], &[bump_seed]]], 
		)?;

		Self::close_pda_and_escrow(
			pda_temp_token_account_info,
			token_program_account_info,
			initializers_main_account_info,
			pda,
			bump_seed,
			pda_account_info,
			escrow_account_info
		)
	}

	/// Cancel can be called after init_escrow. If called after exchange, it's already too late
	/// since exchange is atomic and tokens have been transferred, so there will be no effect.
	/// Since tokens haven't actually been transferred from initializer main token account to
	/// initializer temp token account, we don't need to actually transfer any tokens.
	/// What we need to do is:
	/// 1) Close PDA account
	/// 2) Close escrow info
	fn process_cancel(account_infos: &[AccountInfo], program_id: &Pubkey) -> ProgramResult {
		let account_info_iter = &mut account_infos.iter();
		let initializer_info = next_account_info(account_info_iter)?;

		if !initializer_info.is_signer {
			return Err(ProgramError::MissingRequiredSignature);
		}

		let initializer_token_account_info = next_account_info(account_info_iter)?;
		if *initializer_token_account_info.owner != spl_token::id() {
			return Err(ProgramError::IncorrectProgramId);
		}

		let escrow_account_info = next_account_info(account_info_iter)?;
		let escrow = Escrow::unpack_unchecked(&escrow_account_info.try_borrow_data()?)?;
		if !escrow.is_initialized() {
			return Err(ProgramError::UninitializedAccount);
		}

		let token_program_account_info = next_account_info(account_info_iter)?;

		let pda_temp_token_account_info = next_account_info(account_info_iter)?;
		let pda_temp_token_account = TokenAccount::unpack(&pda_temp_token_account_info.try_borrow_data()?)?;
		let (pda, bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);

		let initializers_main_account_info = next_account_info(account_info_iter)?;
		// Validate Escrow matches instruction
		if escrow.temp_token_account_pubkey != *pda_temp_token_account_info.key {
			return Err(ProgramError::InvalidAccountData);
		}
		if escrow.initializer_pubkey != *initializers_main_account_info.key {
			return Err(ProgramError::InvalidAccountData);
		}
		if escrow.initializer_token_to_receive_account_pubkey != *initializer_token_account_info.key {
			return Err(ProgramError::InvalidAccountData);
		}
		
		let pda_account_info = next_account_info(account_info_iter)?;

		// Close initializer_token_account, return rent fees
		**initializers_main_account_info.lamports.borrow_mut() = initializers_main_account_info.lamports()
			.checked_add(initializer_token_account_info.lamports())
			.ok_or(EscrowError::AmountOverflow)?;
		**initializer_token_account_info.lamports.borrow_mut() = 0;
		*initializer_token_account_info.try_borrow_mut_data()? = &mut [];

		Self::close_pda_and_escrow(
			pda_temp_token_account_info,
			token_program_account_info,
			initializers_main_account_info,
			pda,
			bump_seed,
			pda_account_info,
			escrow_account_info
		)
	}

	fn close_pda_and_escrow<'a>(
		pda_temp_token_account_info: &AccountInfo<'a>,
		token_program_info: &AccountInfo<'a>,
		initializers_main_account_info: &AccountInfo<'a>,
		pda: Pubkey,
		bump_seed: u8,
		pda_account_info: &AccountInfo<'a>,
		escrow_account_info: &AccountInfo<'a>,
	) -> ProgramResult {
		// Close PDA
		let close_pdas_temp_acc_ix = spl_token::instruction::close_account(
			token_program_info.key,
			pda_temp_token_account_info.key,
			initializers_main_account_info.key,
			&pda,
			&[&pda]
		)?;
		msg!("Calling the token program to close pda's temp account...");
		invoke_signed(
			&close_pdas_temp_acc_ix,
			&[
				pda_temp_token_account_info.clone(),
				initializers_main_account_info.clone(),
				pda_account_info.clone(),
				token_program_info.clone(),
			],
			&[&[&b"escrow"[..], &[bump_seed]]],
		)?;

		msg!("Closing the escrow account...");
		**initializers_main_account_info.lamports.borrow_mut() = initializers_main_account_info.lamports()
			.checked_add(escrow_account_info.lamports())
			.ok_or(EscrowError::AmountOverflow)?;
		**escrow_account_info.lamports.borrow_mut() = 0;
		*escrow_account_info.try_borrow_mut_data()? = &mut [];

		Ok(())
	}
}