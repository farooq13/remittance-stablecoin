//! Confidential Token-2022 CPIs. Clients generate proofs and pass verified context accounts.
use anchor_lang::prelude::*;
use spl_token_2022::extension::confidential_transfer::DecryptableBalance;
use spl_token_2022_interface as spl_token_2022;
use spl_token_confidential_transfer_proof_extraction::instruction::ProofLocation;

use super::{
    mint::{invoke_token_instruction, read_decryptable_balance, read_pod},
    token_ops::mint_decimals,
};

/// Configures an already-created token account; only its owner may authorize setup.
#[derive(Accounts)]
pub struct ConfigureConfidentialAccount<'info> {
    /// CHECK: Token-2022 validates this account's mint and owner during configuration.
    #[account(mut)]
    pub token_account: UncheckedAccount<'info>,
    /// CHECK: The Token-2022 CPI validates the mint and its confidential-transfer extension.
    pub mint: UncheckedAccount<'info>,
    pub owner: Signer<'info>,
    /// CHECK: Token-2022 validates this proof context account.
    pub pubkey_validity_proof: UncheckedAccount<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

/// Lets the mint's confidential-transfer authority enable a configured account.
#[derive(Accounts)]
pub struct ApproveConfidentialAccount<'info> {
    /// CHECK: Token-2022 validates that this account is configured for the supplied mint.
    #[account(mut)]
    pub token_account: UncheckedAccount<'info>,
    /// CHECK: The Token-2022 CPI validates the mint and its approval authority.
    pub mint: UncheckedAccount<'info>,
    pub confidential_authority: Signer<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

/// Owner-authorized operations on an already configured confidential token account.
#[derive(Accounts)]
pub struct ConfidentialAccountOwner<'info> {
    /// CHECK: Token-2022 validates this account's mint, owner, and confidential state.
    #[account(mut)]
    pub token_account: UncheckedAccount<'info>,
    /// CHECK: The Token-2022 CPI validates the mint against the token account.
    pub mint: UncheckedAccount<'info>,
    pub owner: Signer<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

/// Transfers with the proof contexts required by a fee-bearing confidential mint.
#[derive(Accounts)]
pub struct ConfidentialTransfer<'info> {
    /// CHECK: Token-2022 validates source ownership, mint, and confidential state.
    #[account(mut)]
    pub source: UncheckedAccount<'info>,
    /// CHECK: The Token-2022 CPI validates the mint against both token accounts.
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates destination ownership, mint, and confidential state.
    #[account(mut)]
    pub destination: UncheckedAccount<'info>,
    pub owner: Signer<'info>,
    /// CHECK: Token-2022 validates this equality proof context account.
    pub equality_proof: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates this ciphertext validity proof context account.
    pub transfer_validity_proof: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates this fee sigma proof context account.
    pub fee_sigma_proof: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates this fee ciphertext validity proof context account.
    pub fee_ciphertext_validity_proof: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates this range proof context account.
    pub range_proof: UncheckedAccount<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

/// Withdraws using proof contexts after first applying the account's pending balance.
#[derive(Accounts)]
pub struct ConfidentialWithdraw<'info> {
    /// CHECK: Token-2022 validates this account's mint, owner, and confidential state.
    #[account(mut)]
    pub token_account: UncheckedAccount<'info>,
    /// CHECK: The Token-2022 CPI validates the mint against the token account.
    pub mint: UncheckedAccount<'info>,
    pub owner: Signer<'info>,
    /// CHECK: Token-2022 validates this equality proof context account.
    pub equality_proof: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates this range proof context account.
    pub range_proof: UncheckedAccount<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ConfigureConfidentialArgs {
    /// Owner-encrypted zero balance used to initialize the confidential account.
    pub encrypted_zero_balance: Vec<u8>,
    /// Maximum pending credits allowed before the owner must apply the balance.
    pub pending_credit_limit: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ApplyPendingArgs {
    /// Expected pending-credit counter after the deposits and transfers being applied.
    pub pending_credits: u64,
    /// New owner-decryptable available balance after applying pending credits.
    pub new_decryptable_balance: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ConfidentialTransferArgs {
    /// Updated owner-decryptable source balance after the transfer.
    pub new_source_decryptable_balance: Vec<u8>,
    /// Auditor ciphertext components bound to the confidential transfer proof.
    pub auditor_ciphertext_lo: Vec<u8>,
    pub auditor_ciphertext_hi: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ConfidentialWithdrawArgs {
    /// Expected pending-credit counter to apply before withdrawing.
    pub pending_credits: u64,
    /// New decryptable available balance after applying those pending credits.
    pub balance_after_apply: Vec<u8>,
    /// Withdrawal amount in token base units.
    pub amount: u64,
    /// New decryptable available balance after the withdrawal.
    pub balance_after_withdraw: Vec<u8>,
}

pub fn handle_configure_confidential_account(
    ctx: Context<ConfigureConfidentialAccount>,
    args: ConfigureConfidentialArgs,
) -> Result<()> {
    let zero_balance = read_decryptable_balance(&args.encrypted_zero_balance)?;
    require!(
        args.pending_credit_limit > 0,
        crate::error::ErrorCode::InvalidPendingCreditLimit
    );
    let pending_credit_limit = args.pending_credit_limit;
    let proof_key = ctx.accounts.pubkey_validity_proof.key();
    let instruction =
        spl_token_2022::extension::confidential_transfer::instruction::inner_configure_account(
            &spl_token_2022::ID,
            &ctx.accounts.token_account.key(),
            &ctx.accounts.mint.key(),
            &zero_balance,
            pending_credit_limit,
            &ctx.accounts.owner.key(),
            &[],
            ProofLocation::ContextStateAccount(&proof_key),
        )?;
    invoke_token_instruction(
        &instruction,
        &ctx.accounts.token_program,
        &[
            ctx.accounts.token_account.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.pubkey_validity_proof.to_account_info(),
            ctx.accounts.owner.to_account_info(),
        ],
    )
}

pub fn handle_approve_confidential_account(ctx: Context<ApproveConfidentialAccount>) -> Result<()> {
    let instruction =
        spl_token_2022::extension::confidential_transfer::instruction::approve_account(
            &spl_token_2022::ID,
            &ctx.accounts.token_account.key(),
            &ctx.accounts.mint.key(),
            &ctx.accounts.confidential_authority.key(),
            &[],
        )?;
    invoke_token_instruction(
        &instruction,
        &ctx.accounts.token_program,
        &[
            ctx.accounts.token_account.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.confidential_authority.to_account_info(),
        ],
    )
}

pub fn handle_deposit_confidential(
    ctx: Context<ConfidentialAccountOwner>,
    amount: u64,
) -> Result<()> {
    let decimals = mint_decimals(&ctx.accounts.mint)?;
    let instruction = spl_token_2022::extension::confidential_transfer::instruction::deposit(
        &spl_token_2022::ID,
        &ctx.accounts.token_account.key(),
        &ctx.accounts.mint.key(),
        amount,
        decimals,
        &ctx.accounts.owner.key(),
        &[],
    )?;
    invoke_token_instruction(
        &instruction,
        &ctx.accounts.token_program,
        &[
            ctx.accounts.token_account.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.owner.to_account_info(),
        ],
    )
}

pub fn handle_apply_pending_balance(
    ctx: Context<ConfidentialAccountOwner>,
    args: ApplyPendingArgs,
) -> Result<()> {
    let new_balance: DecryptableBalance = read_decryptable_balance(&args.new_decryptable_balance)?;
    let instruction =
        spl_token_2022::extension::confidential_transfer::instruction::apply_pending_balance(
            &spl_token_2022::ID,
            &ctx.accounts.token_account.key(),
            args.pending_credits,
            &new_balance,
            &ctx.accounts.owner.key(),
            &[],
        )?;
    invoke_token_instruction(
        &instruction,
        &ctx.accounts.token_program,
        &[
            ctx.accounts.token_account.to_account_info(),
            ctx.accounts.owner.to_account_info(),
        ],
    )
}

pub fn handle_confidential_transfer(
    ctx: Context<ConfidentialTransfer>,
    args: ConfidentialTransferArgs,
) -> Result<()> {
    let new_source_balance: DecryptableBalance =
        read_decryptable_balance(&args.new_source_decryptable_balance)?;
    let auditor_ciphertext_lo = read_pod::<
        spl_token_2022::solana_zk_sdk::encryption::pod::elgamal::PodElGamalCiphertext,
    >(&args.auditor_ciphertext_lo)?;
    let auditor_ciphertext_hi = read_pod::<
        spl_token_2022::solana_zk_sdk::encryption::pod::elgamal::PodElGamalCiphertext,
    >(&args.auditor_ciphertext_hi)?;
    let equality_key = ctx.accounts.equality_proof.key();
    let transfer_key = ctx.accounts.transfer_validity_proof.key();
    let fee_sigma_key = ctx.accounts.fee_sigma_proof.key();
    let fee_ciphertext_key = ctx.accounts.fee_ciphertext_validity_proof.key();
    let range_key = ctx.accounts.range_proof.key();
    let proof_equality = ProofLocation::ContextStateAccount(&equality_key);
    let proof_transfer = ProofLocation::ContextStateAccount(&transfer_key);
    let proof_fee_sigma = ProofLocation::ContextStateAccount(&fee_sigma_key);
    let proof_fee_ciphertext = ProofLocation::ContextStateAccount(&fee_ciphertext_key);
    let proof_range = ProofLocation::ContextStateAccount(&range_key);
    // TransferFeeConfig requires the confidential TransferWithFee instruction for non-self transfers.
    let instruction =
        spl_token_2022::extension::confidential_transfer::instruction::inner_transfer_with_fee(
            &spl_token_2022::ID,
            &ctx.accounts.source.key(),
            &ctx.accounts.mint.key(),
            &ctx.accounts.destination.key(),
            &new_source_balance,
            &auditor_ciphertext_lo,
            &auditor_ciphertext_hi,
            &ctx.accounts.owner.key(),
            &[],
            proof_equality,
            proof_transfer,
            proof_fee_sigma,
            proof_fee_ciphertext,
            proof_range,
        )?;
    invoke_token_instruction(
        &instruction,
        &ctx.accounts.token_program,
        &[
            ctx.accounts.source.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.destination.to_account_info(),
            ctx.accounts.equality_proof.to_account_info(),
            ctx.accounts.transfer_validity_proof.to_account_info(),
            ctx.accounts.fee_sigma_proof.to_account_info(),
            ctx.accounts.fee_ciphertext_validity_proof.to_account_info(),
            ctx.accounts.range_proof.to_account_info(),
            ctx.accounts.owner.to_account_info(),
        ],
    )
}

pub fn handle_withdraw_confidential(
    ctx: Context<ConfidentialWithdraw>,
    args: ConfidentialWithdrawArgs,
) -> Result<()> {
    // ApplyPendingBalance and Withdraw execute atomically in this program instruction.
    let applied_balance = read_decryptable_balance(&args.balance_after_apply)?;
    let instruction =
        spl_token_2022::extension::confidential_transfer::instruction::apply_pending_balance(
            &spl_token_2022::ID,
            &ctx.accounts.token_account.key(),
            args.pending_credits,
            &applied_balance,
            &ctx.accounts.owner.key(),
            &[],
        )?;
    invoke_token_instruction(
        &instruction,
        &ctx.accounts.token_program,
        &[
            ctx.accounts.token_account.to_account_info(),
            ctx.accounts.owner.to_account_info(),
        ],
    )?;

    let new_balance = read_decryptable_balance(&args.balance_after_withdraw)?;
    let decimals = mint_decimals(&ctx.accounts.mint)?;
    let equality_key = ctx.accounts.equality_proof.key();
    let range_key = ctx.accounts.range_proof.key();
    let equality_location = ProofLocation::ContextStateAccount(&equality_key);
    let range_location = ProofLocation::ContextStateAccount(&range_key);
    let instruction =
        spl_token_2022::extension::confidential_transfer::instruction::inner_withdraw(
            &spl_token_2022::ID,
            &ctx.accounts.token_account.key(),
            &ctx.accounts.mint.key(),
            args.amount,
            decimals,
            &new_balance,
            &ctx.accounts.owner.key(),
            &[],
            equality_location,
            range_location,
        )?;
    invoke_token_instruction(
        &instruction,
        &ctx.accounts.token_program,
        &[
            ctx.accounts.token_account.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.equality_proof.to_account_info(),
            ctx.accounts.range_proof.to_account_info(),
            ctx.accounts.owner.to_account_info(),
        ],
    )
}
