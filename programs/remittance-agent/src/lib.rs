use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("2RxSzAKT8HVc3ynLrYPJh4PJVoYC9PsStGkMrBiutgAD");

const DELEGATE_SEED: &[u8] = b"transfer-agent";

#[program]
pub mod remittance_agent {
    use super::*;

    /// Executes a checked transfer using the allowance granted to this user's PDA.
    ///
    /// The source owner does not need to sign this instruction. Token-2022 enforces
    /// the delegated allowance, while the PDA signs only for this program.
    pub fn execute_transfer(ctx: Context<ExecuteTransfer>, amount: u64) -> Result<()> {
        let owner = ctx.accounts.owner.key();
        let bump = [ctx.bumps.delegate];
        let signer_seeds: &[&[u8]] = &[DELEGATE_SEED, owner.as_ref(), &bump];

        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.source.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.destination.to_account_info(),
                    authority: ctx.accounts.delegate.to_account_info(),
                },
                &[signer_seeds],
            ),
            amount,
            ctx.accounts.mint.decimals,
        )
    }
}

#[derive(Accounts)]
pub struct ExecuteTransfer<'info> {
    #[account(
        mut,
        token::mint = mint,
        token::authority = owner,
        token::token_program = token_program
    )]
    pub source: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        token::mint = mint,
        token::token_program = token_program
    )]
    pub destination: InterfaceAccount<'info, TokenAccount>,
    pub mint: InterfaceAccount<'info, Mint>,
    /// CHECK: This PDA is the delegate signer; Token-2022 checks its allowance.
    #[account(seeds = [DELEGATE_SEED, owner.key().as_ref()], bump)]
    pub delegate: UncheckedAccount<'info>,
    /// CHECK: Its key is constrained as the source token account's owner.
    pub owner: UncheckedAccount<'info>,
    #[account(address = anchor_spl::token_2022::ID)]
    pub token_program: Interface<'info, TokenInterface>,
}

pub fn delegate_address(owner: Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[DELEGATE_SEED, owner.as_ref()], &ID)
}
