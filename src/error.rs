// Using this library so we don't need to implement fmt::Display for our new Error types 
// (https://doc.rust-lang.org/rust-by-example/error/multiple_error_types/define_error_type.html)
use thiserror::Error; 

use solana_program::program_error::ProgramError;

#[derive(Error, Debug, Copy, Clone)]
pub enum EscrowError {
	// Invalid Instruction
	#[error("Invalid Instruction")]
	InvalidInstruction,
	#[error("Not Rent Exempt")]
	NotRentExempt,
}

impl From<EscrowError> for ProgramError {
	fn from(e: EscrowError) -> Self {
		ProgramError::Custom(e as u32)
	}
}