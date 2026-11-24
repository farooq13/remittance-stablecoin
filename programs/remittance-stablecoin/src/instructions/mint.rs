use anchor_lang::{prelude::*, solana_program::program::invoke};
use bytemuck::try_pod_read_unaligned;
use spl_token_2022::{
    extension::{confidential_transfer::DecryptableBalance, ExtensionType},
    state::{AccountState, Mint},
};
use spl_token_2022_interface as spl_token_2022;
use spl_token_metadata_interface::state::TokenMetadata;

use crate::error::ErrorCode as ProgramErrorCode;

/// Configuration shared by the initial and confidential mint issuances.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MintArgs {
    /// Number of decimal places supported by the token.
    pub decimals: u8,
    /// Transfer fee in basis points, capped at 10,000.
    pub transfer_fee_basis_points: u16,
    /// Maximum fee charged per transfer, in base units.
    pub maximum_fee: u64,
    /// On-chain token display name.
    pub name: String,
    /// On-chain token ticker.
    pub symbol: String,
    /// Metadata URI stored alongside the name and ticker.
    pub uri: String,
}

/// Additional confidential-fee key material for a replacement mint.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ConfidentialMintArgs {
    pub mint: MintArgs,
    /// ElGamal public key used to encrypt confidential withheld fees.
    pub confidential_fee_elgamal_pubkey: Vec<u8>,
}

#[derive(Accounts)]
pub struct InitializeMint<'info> {
    #[account(mut)]
    pub issuer: Signer<'info>,
    #[account(mut)]
    pub mint: Signer<'info>,
    /// CHECK: The address is constrained to the canonical Token-2022 program.
    #[account(address = spl_token_2022::ID)]
    pub token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn handle_initialize_mint(ctx: Context<InitializeMint>, args: MintArgs) -> Result<()> {
    create_mint(
        &ctx.accounts.issuer,
        &ctx.accounts.mint,
        &ctx.accounts.token_program,
        &ctx.accounts.system_program,
        args,
        None,
    )
}

pub fn handle_reissue_confidential_mint(
    ctx: Context<InitializeMint>,
    args: ConfidentialMintArgs,
) -> Result<()> {
    // A reissue is a distinct mint; balances from the old mint require holder-authorized migration.
    let confidential_fee_key = read_pod::<
        spl_token_2022::solana_zk_sdk::encryption::pod::elgamal::PodElGamalPubkey,
    >(&args.confidential_fee_elgamal_pubkey)?;
    create_mint(
        &ctx.accounts.issuer,
        &ctx.accounts.mint,
        &ctx.accounts.token_program,
        &ctx.accounts.system_program,
        args.mint,
        Some(confidential_fee_key),
    )
}

fn create_mint<'info>(
    issuer: &Signer<'info>,
    mint: &Signer<'info>,
    token_program: &UncheckedAccount<'info>,
    system_program: &Program<'info, System>,
    args: MintArgs,
    confidential_fee_key: Option<
        spl_token_2022::solana_zk_sdk::encryption::pod::elgamal::PodElGamalPubkey,
    >,
) -> Result<()> {
    require!(
        args.transfer_fee_basis_points <= 10_000,
        ProgramErrorCode::InvalidMintConfiguration
    );

    let issuer_key = issuer.key();
    let mint_key = mint.key();
    let token_program_id = spl_token_2022::ID;
    let mut extensions = vec![
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
    ];
    if confidential_fee_key.is_some() {
        extensions.extend([
            ExtensionType::PermanentDelegate,
            ExtensionType::ConfidentialTransferMint,
            ExtensionType::ConfidentialTransferFeeConfig,
        ]);
    }

    let mint_space = ExtensionType::try_calculate_account_len::<Mint>(&extensions)?;
    let metadata = TokenMetadata {
        name: args.name.clone(),
        symbol: args.symbol.clone(),
        uri: args.uri.clone(),
        ..TokenMetadata::default()
    };
    let final_space = mint_space
        .checked_add(metadata.tlv_size_of()?)
        .ok_or(ProgramErrorCode::InvalidMintConfiguration)?;
    let lamports = Rent::get()?.minimum_balance(final_space);

    // Reserve rent for variable-length metadata; its extension is allocated during metadata initialization.
    anchor_lang::system_program::create_account(
        CpiContext::new(
            system_program.key(),
            anchor_lang::system_program::CreateAccount {
                from: issuer.to_account_info(),
                to: mint.to_account_info(),
            },
        ),
        lamports,
        mint_space as u64,
        &token_program_id,
    )?;

    // Token-2022 requires fixed mint extensions to be initialized before InitializeMint2.
    invoke_token_instruction(
        &spl_token_2022::extension::transfer_fee::instruction::initialize_transfer_fee_config(
            &token_program_id,
            &mint_key,
            Some(&issuer_key),
            Some(&issuer_key),
            args.transfer_fee_basis_points,
            args.maximum_fee,
        )?,
        token_program,
        &[mint.to_account_info()],
    )?;
    invoke_token_instruction(
        &spl_token_2022::extension::metadata_pointer::instruction::initialize(
            &token_program_id,
            &mint_key,
            Some(issuer_key),
            Some(mint_key),
        )?,
        token_program,
        &[mint.to_account_info()],
    )?;
    invoke_token_instruction(
        &spl_token_2022::extension::default_account_state::instruction::initialize_default_account_state(
            &token_program_id,
            &mint_key,
            &AccountState::Frozen,
        )?,
        token_program,
        &[mint.to_account_info()],
    )?;
    invoke_token_instruction(
        &spl_token_2022::instruction::initialize_mint_close_authority(
            &token_program_id,
            &mint_key,
            Some(&issuer_key),
        )?,
        token_program,
        &[mint.to_account_info()],
    )?;

    if let Some(fee_key) = confidential_fee_key {
        invoke_token_instruction(
            &spl_token_2022::instruction::initialize_permanent_delegate(
                &token_program_id,
                &mint_key,
                &issuer_key,
            )?,
            token_program,
            &[mint.to_account_info()],
        )?;
        invoke_token_instruction(
            &spl_token_2022::extension::confidential_transfer::instruction::initialize_mint(
                &token_program_id,
                &mint_key,
                Some(issuer_key),
                // Manual policy: each owner configures first, then the issuer approves.
                false,
                None,
            )?,
            token_program,
            &[mint.to_account_info()],
        )?;
        invoke_token_instruction(
            &spl_token_2022::extension::confidential_transfer_fee::instruction::initialize_confidential_transfer_fee_config(
                &token_program_id,
                &mint_key,
                Some(issuer_key),
                &fee_key,
            )?,
            token_program,
            &[mint.to_account_info()],
        )?;
    }

    invoke_token_instruction(
        &spl_token_2022::instruction::initialize_mint2(
            &token_program_id,
            &mint_key,
            &issuer_key,
            Some(&issuer_key),
            args.decimals,
        )?,
        token_program,
        &[mint.to_account_info()],
    )?;

    // TokenMetadata initialization requires an initialized mint and metadata pointer.
    let metadata_ix = spl_token_metadata_interface::instruction::initialize(
        &token_program_id,
        &mint_key,
        &issuer_key,
        &mint_key,
        &issuer_key,
        args.name,
        args.symbol,
        args.uri,
    );
    invoke_token_instruction(
        &metadata_ix,
        token_program,
        &[
            mint.to_account_info(),
            issuer.to_account_info(),
            mint.to_account_info(),
            issuer.to_account_info(),
        ],
    )?;

    msg!(
        "Token-2022 mint initialized: {}, confidential={}",
        mint_key,
        confidential_fee_key.is_some()
    );
    Ok(())
}

pub(crate) fn invoke_token_instruction<'info>(
    instruction: &anchor_lang::solana_program::instruction::Instruction,
    token_program: &UncheckedAccount<'info>,
    accounts: &[AccountInfo<'info>],
) -> Result<()> {
    // CPI account infos follow the instruction's metas; the invoked program account is appended.
    let mut account_infos = accounts.to_vec();
    account_infos.push(token_program.to_account_info());
    invoke(instruction, &account_infos)?;
    Ok(())
}

pub(crate) fn read_pod<T: bytemuck::Pod>(bytes: &[u8]) -> Result<T> {
    // Reject malformed client-supplied ciphertexts instead of accepting truncated proof data.
    try_pod_read_unaligned(bytes).map_err(|_| error!(ProgramErrorCode::InvalidProofData))
}

pub(crate) fn read_decryptable_balance(bytes: &[u8]) -> Result<DecryptableBalance> {
    read_pod(bytes)
}
