use anchor_lang::prelude::*;
use spl_token_2022::{
    extension::{transfer_fee::TransferFeeConfig, BaseStateWithExtensions, StateWithExtensions},
    state::Mint,
};
use spl_token_2022_interface as spl_token_2022;

use super::mint::{invoke_token_instruction, read_decryptable_balance};
use crate::error::ErrorCode;
use spl_token_confidential_transfer_proof_extraction::instruction::ProofLocation;

#[derive(Accounts)]
pub struct TransferCheckedWithFee<'info> {
    /// CHECK: Token-2022 validates source ownership, mint, and transfer state.
    #[account(mut)]
    pub source: UncheckedAccount<'info>,
    /// CHECK: The instruction reads fee configuration and decimals through extension-aware state.
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates destination ownership, mint, and transfer state.
    #[account(mut)]
    pub destination: UncheckedAccount<'info>,
    pub authority: Signer<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct ThawKycAccount<'info> {
    /// CHECK: Token-2022 validates this mint's freeze authority and account association.
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates the account belongs to the mint and is frozen.
    #[account(mut)]
    pub token_account: UncheckedAccount<'info>,
    pub freeze_authority: Signer<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct SeizePublicBalance<'info> {
    /// CHECK: Token-2022 validates the permanent delegate and source account.
    #[account(mut)]
    pub source: UncheckedAccount<'info>,
    /// CHECK: The Token-2022 CPI validates the mint and its permanent-delegate extension.
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates destination ownership and mint association.
    #[account(mut)]
    pub destination: UncheckedAccount<'info>,
    pub permanent_delegate: Signer<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CloseMint<'info> {
    /// CHECK: Token-2022 validates the mint's close authority and zero-supply condition.
    #[account(mut)]
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates this writable lamport destination.
    #[account(mut)]
    pub destination: UncheckedAccount<'info>,
    pub close_authority: Signer<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CollectFees<'info> {
    /// CHECK: Token-2022 validates transfer-fee configuration and source accounts.
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates this fee withdrawal destination.
    #[account(mut)]
    pub destination: UncheckedAccount<'info>,
    pub withdraw_authority: Signer<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct HarvestConfidentialFees<'info> {
    /// CHECK: Token-2022 validates confidential fee state and supplied sources.
    #[account(mut)]
    pub mint: UncheckedAccount<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct WithdrawConfidentialFees<'info> {
    /// CHECK: Token-2022 validates confidential fee state and the proof context.
    #[account(mut)]
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Token-2022 validates the withdrawal destination.
    #[account(mut)]
    pub destination: UncheckedAccount<'info>,
    pub withdraw_authority: Signer<'info>,
    /// CHECK: Token-2022 validates this equality proof context account.
    pub equality_proof: UncheckedAccount<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct WithdrawConfidentialFeesArgs {
    pub new_decryptable_balance: Vec<u8>,
}

pub fn handle_transfer_checked_with_fee(
    ctx: Context<TransferCheckedWithFee>,
    amount: u64,
) -> Result<()> {
    transfer_with_fee(
        &ctx.accounts.source,
        &ctx.accounts.mint,
        &ctx.accounts.destination,
        &ctx.accounts.authority,
        &ctx.accounts.token_program,
        amount,
    )
}

pub fn handle_seize_public_balance(ctx: Context<SeizePublicBalance>, amount: u64) -> Result<()> {
    // PermanentDelegate can move the public base balance, but cannot decrypt confidential balances.
    transfer_with_fee(
        &ctx.accounts.source,
        &ctx.accounts.mint,
        &ctx.accounts.destination,
        &ctx.accounts.permanent_delegate,
        &ctx.accounts.token_program,
        amount,
    )
}

pub(crate) fn mint_decimals(mint: &UncheckedAccount<'_>) -> Result<u8> {
    let mint_data = mint.try_borrow_data()?;
    let mint_state = StateWithExtensions::<Mint>::unpack(&mint_data)?;
    Ok(mint_state.base.decimals)
}

fn transfer_with_fee<'info>(
    source: &UncheckedAccount<'info>,
    mint: &UncheckedAccount<'info>,
    destination: &UncheckedAccount<'info>,
    authority: &Signer<'info>,
    token_program: &UncheckedAccount<'info>,
    amount: u64,
) -> Result<()> {
    let mint_data = mint.try_borrow_data()?;
    let mint_state = StateWithExtensions::<Mint>::unpack(&mint_data)?;
    let fee_config = mint_state
        .get_extension::<TransferFeeConfig>()
        .map_err(|_| error!(ErrorCode::MissingTransferFeeConfig))?;
    // Read the active schedule each time so scheduled epoch changes are honored.
    let epoch = Clock::get()?.epoch;
    let expected_fee = fee_config
        .calculate_epoch_fee(epoch, amount)
        .ok_or(ErrorCode::FeeCalculationFailed)?;
    let decimals = mint_state.base.decimals;
    drop(mint_data);

    let instruction =
        spl_token_2022::extension::transfer_fee::instruction::transfer_checked_with_fee(
            &spl_token_2022::ID,
            &source.key(),
            &mint.key(),
            &destination.key(),
            &authority.key(),
            &[],
            amount,
            decimals,
            expected_fee,
        )?;
    invoke_token_instruction(
        &instruction,
        token_program,
        &[
            source.to_account_info(),
            mint.to_account_info(),
            destination.to_account_info(),
            authority.to_account_info(),
        ],
    )
}

pub fn handle_thaw_kyc_account(ctx: Context<ThawKycAccount>) -> Result<()> {
    let instruction = spl_token_2022::instruction::thaw_account(
        &spl_token_2022::ID,
        &ctx.accounts.token_account.key(),
        &ctx.accounts.mint.key(),
        &ctx.accounts.freeze_authority.key(),
        &[],
    )?;
    invoke_token_instruction(
        &instruction,
        &ctx.accounts.token_program,
        &[
            ctx.accounts.token_account.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.freeze_authority.to_account_info(),
        ],
    )
}

pub fn handle_close_mint(ctx: Context<CloseMint>) -> Result<()> {
    // Token-2022 enforces zero supply and all other mint-close preconditions.
    let instruction = spl_token_2022::instruction::close_account(
        &spl_token_2022::ID,
        &ctx.accounts.mint.key(),
        &ctx.accounts.destination.key(),
        &ctx.accounts.close_authority.key(),
        &[],
    )?;
    invoke_token_instruction(
        &instruction,
        &ctx.accounts.token_program,
        &[
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.destination.to_account_info(),
            ctx.accounts.close_authority.to_account_info(),
        ],
    )
}

pub fn handle_collect_fees<'info>(ctx: Context<'info, CollectFees<'info>>) -> Result<()> {
    // The fee instruction appends source accounts after mint, destination, and authority.
    let source_accounts = ctx.remaining_accounts;
    require!(!source_accounts.is_empty(), ErrorCode::MissingFeeSources);

    let source_keys: Vec<Pubkey> = source_accounts.iter().map(|account| *account.key).collect();
    let source_key_refs: Vec<&Pubkey> = source_keys.iter().collect();
    let instruction =
        spl_token_2022::extension::transfer_fee::instruction::withdraw_withheld_tokens_from_accounts(
            &spl_token_2022::ID,
            &ctx.accounts.mint.key(),
            &ctx.accounts.destination.key(),
            &ctx.accounts.withdraw_authority.key(),
            &[],
            &source_key_refs,
        )?;
    let mut account_infos = vec![
        ctx.accounts.mint.to_account_info(),
        ctx.accounts.destination.to_account_info(),
        ctx.accounts.withdraw_authority.to_account_info(),
    ];
    account_infos.extend(source_accounts.iter().cloned());
    invoke_token_instruction(&instruction, &ctx.accounts.token_program, &account_infos)
}

pub fn handle_harvest_confidential_fees<'info>(
    ctx: Context<'info, HarvestConfidentialFees<'info>>,
) -> Result<()> {
    // Harvest is permissionless; only the later withdrawal requires the configured authority.
    let sources = ctx.remaining_accounts;
    require!(!sources.is_empty(), ErrorCode::MissingFeeSources);
    let source_keys: Vec<Pubkey> = sources.iter().map(|account| *account.key).collect();
    let source_key_refs: Vec<&Pubkey> = source_keys.iter().collect();
    let instruction =
        spl_token_2022::extension::confidential_transfer_fee::instruction::harvest_withheld_tokens_to_mint(
            &spl_token_2022::ID,
            &ctx.accounts.mint.key(),
            &source_key_refs,
        )?;
    let mut account_infos = vec![ctx.accounts.mint.to_account_info()];
    account_infos.extend(sources.iter().cloned());
    invoke_token_instruction(&instruction, &ctx.accounts.token_program, &account_infos)
}

pub fn handle_withdraw_confidential_fees(
    ctx: Context<WithdrawConfidentialFees>,
    args: WithdrawConfidentialFeesArgs,
) -> Result<()> {
    let new_balance = read_decryptable_balance(&args.new_decryptable_balance)?;
    let proof_key = ctx.accounts.equality_proof.key();
    let instruction =
        spl_token_2022::extension::confidential_transfer_fee::instruction::inner_withdraw_withheld_tokens_from_mint(
            &spl_token_2022::ID,
            &ctx.accounts.mint.key(),
            &ctx.accounts.destination.key(),
            &new_balance,
            &ctx.accounts.withdraw_authority.key(),
            &[],
            ProofLocation::ContextStateAccount(&proof_key),
        )?;
    invoke_token_instruction(
        &instruction,
        &ctx.accounts.token_program,
        &[
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.destination.to_account_info(),
            ctx.accounts.equality_proof.to_account_info(),
            ctx.accounts.withdraw_authority.to_account_info(),
        ],
    )
}
