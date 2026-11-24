use anchor_lang::prelude::*;

/// Errors for invalid mint parameters, fees, and confidential-proof inputs.
#[error_code]
pub enum ErrorCode {
    #[msg("Confidential proof data has an invalid byte length")]
    InvalidProofData,
    #[msg("Transfer fee extension is missing from this mint")]
    MissingTransferFeeConfig,
    #[msg("Pending balance credit limit must be greater than zero")]
    InvalidPendingCreditLimit,
    #[msg("Mint configuration contains an invalid value")]
    InvalidMintConfiguration,
    #[msg("Current-epoch transfer fee could not be calculated")]
    FeeCalculationFailed,
    #[msg("At least one source account is required to collect fees")]
    MissingFeeSources,
}
