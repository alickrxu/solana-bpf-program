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
			}
		}
	}

	fn process_init_escrow(accounts: &[AccountInfo], amount: u64, program_id: &Pubkey) -> ProgramResult {
		let account_info_iter = &mut accounts.iter();
		let initializer = next_account_info(account_info_iter)?;

		if !initializer.is_signer {
			return Err(ProgramError::MissingRequiredSignature);
		}

		// This program must be owned by the Solana Token Program
		let temp_token_account = next_account_info(account_info_iter)?;

		// This one too, but we actually check it here. Why don't we check previously?
		let token_to_receive_account = next_account_info(account_info_iter)?;
		if *token_to_receive_account.owner != spl_token::id() {
			return Err(ProgramError::IncorrectProgramId);
		}

		let escrow_account = next_account_info(account_info_iter)?;
		let rent = &Rent::from_account_info(next_account_info(account_info_iter)?)?;

		if !rent.is_exempt(escrow_account.lamports(), escrow_account.data_len()) {
			return Err(EscrowError::NotRentExempt.into());
		}

		// unpack_unchecked comes from default functions from trait in program_pack 
		// https://docs.rs/solana-program/latest/src/solana_program/program_pack.rs.html#29-39
		// try_borrow_data fetches the "data" field from the AccountInfo struct
		let mut escrow_info = Escrow::unpack_unchecked(&escrow_account.try_borrow_data()?)?;
		if escrow_info.is_initialized() {
			return Err(ProgramError::AccountAlreadyInitialized);
		}

		// Now that we know escrow struct is uninitialized, let's initialize 
		escrow_info.is_initialized = true;
		escrow_info.initializer_pubkey = *initializer.key;
		escrow_info.temp_token_account_pubkey = *temp_token_account.key;
		escrow_info.initializer_token_to_receive_account_pubkey = *token_to_receive_account.key;
		escrow_info.expected_amount = amount;

		Escrow::pack(escrow_info, &mut escrow_account.try_borrow_mut_data()?)?;

		// Program Derived Address
		// Why do we seed with address of byte array "escrow"? A: It's just good convention. Also makes it easy to refer later on.
		// PDA are NOT on the ed25519 curve, meaning not possible to collide with Solana key pairs
		let (pda, _bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);

		let token_program = next_account_info(account_info_iter)?;
		let owner_change_ix = spl_token::instruction::set_authority(
			token_program.key,
			temp_token_account.key, // set_authority will fail if temp_token_account is not owned by Token program
			Some(&pda),
			spl_token::instruction::AuthorityType::AccountOwner,
			initializer.key,
			&[&initializer.key],
		)?;

		msg!("Calling the token program to transfer token account ownership...");
		invoke( // Calls the token program FROM our escrow program
			&owner_change_ix,
			&[
				temp_token_account.clone(),
				initializer.clone(),
				token_program.clone(),
			]	
		)?;

		Ok(())
	}

	fn process_exchange(accounts: &[AccountInfo], amount_expected_by_taker: u64, program_id: &Pubkey) -> ProgramResult {
		let account_info_iter = &mut accounts.iter();
		let taker = next_account_info(account_info_iter)?;

		if !taker.is_signer {
			return Err(ProgramError::MissingRequiredSignature);
		}	

		let takers_sending_account = next_account_info(account_info_iter)?;
		let takers_token_to_receive_account = next_account_info(account_info_iter)?;

		let pda_temp_token_account = next_account_info(account_info_iter)?;
		let pda_temp_token_account_info = TokenAccount::unpack(&pda_temp_token_account.try_borrow_data()?)?;
		let (pda, bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);

		// Amount validation, prevent frontrunning
		if amount_expected_by_taker != pda_temp_token_account_info.amount {
			return Err(EscrowError::ExpectedAmountMismatch.into()); // TODO why do we need .into?
		}

		let initializers_main_account = next_account_info(account_info_iter)?;
		let initializers_token_to_receive_account = next_account_info(account_info_iter)?;
		let escrow_account = next_account_info(account_info_iter)?;

		let escrow_info = Escrow::unpack(&escrow_account.try_borrow_data()?)?;

		// Validate Escrow matches instruction 
		if escrow_info.temp_token_account_pubkey != *pda_temp_token_account.key {
			return Err(ProgramError::InvalidAccountData);
		}
		if escrow_info.initializer_pubkey != *initializers_main_account.key {
			return Err(ProgramError::InvalidAccountData);
		}
		if escrow_info.initializer_token_to_receive_account_pubkey != *initializers_token_to_receive_account.key {
			return Err(ProgramError::InvalidAccountData);
		}

		let token_program = next_account_info(account_info_iter)?;

		let transfer_to_initializer_ix = spl_token::instruction::transfer(  // TODO do the instructions in spl_token::instruction encompass all possible instructions in solana??
			token_program.key, // token program ID
        	takers_sending_account.key, // source pubkey
        	initializers_token_to_receive_account.key, // destination pubkey
        	taker.key,  // authority pubkey
        	&[&taker.key],  // signer pubkeys
        	escrow_info.expected_amount,
		)?;
		msg!("Calling the token program to transfer tokens to the escrow's initializer...");
		invoke(
			&transfer_to_initializer_ix,
			&[
				takers_sending_account.clone(),
				initializers_token_to_receive_account.clone(),
				taker.clone(),
				token_program.clone()
			]
		)?;

		let pda_account = next_account_info(account_info_iter)?;
		let transfer_to_taker_ix = spl_token::instruction::transfer(
		    token_program.key,
		    pda_temp_token_account.key,
		    takers_token_to_receive_account.key,
		    &pda,
		    &[&pda],
		    pda_temp_token_account_info.amount,
		)?;
		msg!("Calling the token program to transfer tokens to the taker...");
		invoke_signed(
		    &transfer_to_taker_ix,
		    &[
		        pda_temp_token_account.clone(),
		        takers_token_to_receive_account.clone(),
		        pda_account.clone(),
		        token_program.clone(),
		    ],
		    // This parameter is for authority. In this case, the authority is the PDA. BUT instead of passing in the key for PDA itself, we pass in the seeds (&[b"escrow"] and bump_seed), so that we can recalculate the PDA. If the recalculation and the given PDA keys dont' match, then this instruction fails with AuthenticationError
		    &[&[&b"escrow"[..], &[bump_seed]]], 
		)?;

		// Close PDA
		let close_pdas_temp_acc_ix = spl_token::instruction::close_account(
		    token_program.key,
		    pda_temp_token_account.key,
		    initializers_main_account.key,
		    &pda,
		    &[&pda]
		)?;
		msg!("Calling the token program to close pda's temp account...");
		invoke_signed(
		    &close_pdas_temp_acc_ix,
		    &[
		        pda_temp_token_account.clone(),
		        initializers_main_account.clone(),
		        pda_account.clone(),
		        token_program.clone(),
		    ],
		    &[&[&b"escrow"[..], &[bump_seed]]],
		)?;

		msg!("Closing the escrow account...");
		**initializers_main_account.lamports.borrow_mut() = initializers_main_account.lamports()
			.checked_add(escrow_account.lamports())
			.ok_or(EscrowError::AmountOverflow)?;
		**escrow_account.lamports.borrow_mut() = 0;
		*escrow_account.try_borrow_mut_data()? = &mut [];

		Ok(())
	}
}