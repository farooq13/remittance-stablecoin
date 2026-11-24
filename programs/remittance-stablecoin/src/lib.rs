pub mod error;
pub mod instructions;

use anchor_lang::prelude::*;

pub use instructions::*;

declare_id!("E8js5UVXNrdYmGeCCBKE2vdL1qK4XbjkaZ88TJDi19Wz");

/// Token-2022 mint, compliance, transfer-fee, and confidential-transfer entrypoints.
#[program]
pub mod remittance_stablecoin {
    use super::*;

    /// Creates a fee-bearing mint with on-mint metadata and frozen-by-default accounts.
    pub fn initialize_mint(ctx: Context<InitializeMint>, args: MintArgs) -> Result<()> {
        instructions::mint::handle_initialize_mint(ctx, args)
    }

    /// Creates a replacement mint with confidential transfers and a permanent delegate.
    ///
    /// This does not migrate supply or balances from an existing mint.
    pub fn reissue_confidential_mint(
        ctx: Context<InitializeMint>,
        args: ConfidentialMintArgs,
    ) -> Result<()> {
        instructions::mint::handle_reissue_confidential_mint(ctx, args)
    }

    /// Transfers public token balances while enforcing the mint's current-epoch fee.
    pub fn transfer_checked_with_fee(
        ctx: Context<TransferCheckedWithFee>,
        amount: u64,
    ) -> Result<()> {
        instructions::token_ops::handle_transfer_checked_with_fee(ctx, amount)
    }

    /// Thaws one token account after KYC; this does not change the mint's default state.
    pub fn thaw_kyc_account(ctx: Context<ThawKycAccount>) -> Result<()> {
        instructions::token_ops::handle_thaw_kyc_account(ctx)
    }

    /// Uses the permanent delegate to move a public balance from a sanctioned account.
    ///
    /// Confidential balances are not available to this seizure path.
    pub fn seize_public_balance(ctx: Context<SeizePublicBalance>, amount: u64) -> Result<()> {
        instructions::token_ops::handle_seize_public_balance(ctx, amount)
    }

    /// Withdraws transparent withheld fees from the supplied source accounts.
    ///
    /// Pass source token accounts as remaining writable accounts.
    pub fn collect_fees<'info>(ctx: Context<'info, CollectFees<'info>>) -> Result<()> {
        instructions::token_ops::handle_collect_fees(ctx)
    }

    /// Harvests confidential withheld fees from source accounts into the mint.
    ///
    /// Pass source token accounts as remaining writable accounts.
    pub fn harvest_confidential_fees<'info>(
        ctx: Context<'info, HarvestConfidentialFees<'info>>,
    ) -> Result<()> {
        instructions::token_ops::handle_harvest_confidential_fees(ctx)
    }

    /// Withdraws harvested confidential fees using a verified equality-proof context.
    pub fn withdraw_confidential_fees(
        ctx: Context<WithdrawConfidentialFees>,
        args: WithdrawConfidentialFeesArgs,
    ) -> Result<()> {
        instructions::token_ops::handle_withdraw_confidential_fees(ctx, args)
    }

    /// Closes a decommissioned mint when Token-2022's close conditions are met.
    pub fn close_mint(ctx: Context<CloseMint>) -> Result<()> {
        instructions::token_ops::handle_close_mint(ctx)
    }

    /// Configures confidential transfers for an existing token account.
    ///
    /// The token-account owner must sign; supply a verified public-key proof context.
    pub fn configure_confidential_account(
        ctx: Context<ConfigureConfidentialAccount>,
        args: ConfigureConfidentialArgs,
    ) -> Result<()> {
        instructions::confidential::handle_configure_confidential_account(ctx, args)
    }

    /// Manually approves an owner-configured account for confidential transfers.
    pub fn approve_confidential_account(ctx: Context<ApproveConfidentialAccount>) -> Result<()> {
        instructions::confidential::handle_approve_confidential_account(ctx)
    }

    /// Deposits public tokens from the owner's account into its confidential pending balance.
    pub fn deposit_confidential(ctx: Context<ConfidentialAccountOwner>, amount: u64) -> Result<()> {
        instructions::confidential::handle_deposit_confidential(ctx, amount)
    }

    /// Moves pending confidential credits into the owner's available balance.
    pub fn apply_pending_balance(
        ctx: Context<ConfidentialAccountOwner>,
        args: ApplyPendingArgs,
    ) -> Result<()> {
        instructions::confidential::handle_apply_pending_balance(ctx, args)
    }

    /// Transfers confidential tokens and fees using pre-verified proof contexts.
    pub fn confidential_transfer(
        ctx: Context<ConfidentialTransfer>,
        args: ConfidentialTransferArgs,
    ) -> Result<()> {
        instructions::confidential::handle_confidential_transfer(ctx, args)
    }

    /// Applies pending credits, then withdraws confidential tokens to the public balance.
    pub fn withdraw_confidential(
        ctx: Context<ConfidentialWithdraw>,
        args: ConfidentialWithdrawArgs,
    ) -> Result<()> {
        instructions::confidential::handle_withdraw_confidential(ctx, args)
    }
}
