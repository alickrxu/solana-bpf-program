use std::convert::TryInto;
use solana_program::program_error::ProgramError;

use crate::error::EscrowError::InvalidInstruction;

pub enum EscrowInstruction {
	/// Starts the trade by creating and populating an escrow account and transferring ownership of the given temp token account to the PDA
    ///
    /// Accounts expected:
    ///
    /// 0. `[signer]` The account of the person initializing the escrow
    /// 1. `[writable]` Temporary token account that should be created prior to this instruction and owned by the initializer
    /// 2. `[]` The initializer's token account for the token they will receive should the trade go through
    /// 3. `[writable]` The escrow account, it will hold all necessary info about the trade.
    /// 4. `[]` The rent sysvar
    /// 5. `[]` The token program
	InitEscrow {
		/// The amount party A expects to receive of token Y
		amount: u64
	},

	/// Accepts a trade
	/// Accounts expected:
	///
	/// 0. `[signer]` The account of the person taking the trade
	/// 1. `[writable]` The taker's token account for the token they send 
	/// 2. `[writable]` The taker's token account for the token they will receive should the trade go through
	/// 3. `[writable]` The PDA's temp token account to get tokens from and eventually close
	/// 4. `[writable]` The initializer's main account to send their rent fees to
	/// 5. `[writable]` The initializer's token account that will receive tokens
	/// 6. `[writable]` The escrow account holding the escrow info
	/// 7. `[]` The token program
	/// 8. `[]` The PDA account
	Exchange {
		/// the amount the taker expects to be paid in the other token
		amount: u64
	},

	/// Allow initializer to cancel the trade 
	/// Accounts expeted:
	///
	/// 0. `[signer]` The account of the person who initialized the escrow and wants to cancel
	/// 1. `[writable]` The initializer's original token account that should get tokens back
	/// 2. `[writable]` The escrow account, which should be closed after this tx
	/// 3. `[]` The token program
	/// 4. `[writable]` The PDA temp token account that has the tokens to return, should be closed
	/// 5. `[writable]` The initializer's main account to receive rent from escrow and temp token account
	/// 6. `[]` The PDA account
	Cancel {
	}
}

impl EscrowInstruction {
	/// Unpacks a byte buffer into a [EscrowInstruction](enum.EscrowInstruction.html).
	pub fn unpack(input: &[u8]) -> Result<Self, ProgramError> {
		let (tag, rest) = input.split_first().ok_or(InvalidInstruction)?;

		Ok(match tag {
			0 => Self::InitEscrow {
				amount: Self::unpack_amount(rest)?,
			},
			1 => Self::Exchange {
				amount: Self::unpack_amount(rest)?,
			},
			2 => Self::Cancel {},
			_ => return Err(InvalidInstruction.into()),
		})
	}

	fn unpack_amount(input: &[u8]) -> Result<u64, ProgramError> {
		let amount = input
			.get(..8)
			.and_then(|slice| slice.try_into().ok())
			.map(u64::from_le_bytes)
			.ok_or(InvalidInstruction)?;
		Ok(amount)
	}
}