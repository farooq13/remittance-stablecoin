use {
    anchor_lang::{solana_program::instruction::Instruction, InstructionData, ToAccountMetas},
    bytemuck::{bytes_of, Pod},
    litesvm::LiteSVM,
    solana_account::Account,
    solana_keypair::Keypair,
    solana_signer::Signer,
    solana_transaction::Transaction,
    spl_associated_token_account_interface::{
        address::get_associated_token_address_with_program_id,
        instruction::create_associated_token_account,
    },
    spl_token_2022_interface::solana_zk_sdk::{
        encryption::{
            auth_encryption::{AeCiphertext, AeKey},
            elgamal::{ElGamalCiphertext, ElGamalKeypair, ElGamalPubkey},
            pod::{auth_encryption::PodAeCiphertext, elgamal::PodElGamalPubkey},
        },
        zk_elgamal_proof_program::{
            instruction::{ContextStateInfo, ProofInstruction},
            proof_data::{ZkProofData, *},
            state::ProofContextState,
        },
    },
    spl_token_2022_interface::{
        extension::{
            confidential_transfer::{ConfidentialTransferAccount, ConfidentialTransferMint},
            default_account_state::DefaultAccountState,
            metadata_pointer::MetadataPointer,
            mint_close_authority::MintCloseAuthority,
            permanent_delegate::PermanentDelegate,
            transfer_fee::{TransferFeeAmount, TransferFeeConfig},
            BaseStateWithExtensions, ExtensionType, StateWithExtensions,
        },
        state::{Account as TokenAccount, AccountState, Mint},
    },
    spl_token_confidential_transfer_proof_generation::{
        transfer_with_fee::transfer_with_fee_split_proof_data, try_combine_lo_hi_ciphertexts,
        withdraw::withdraw_proof_data,
    },
    spl_token_metadata_interface::state::TokenMetadata,
    std::{fs, path::PathBuf},
};

use remittance_stablecoin::{
    accounts, instruction, instructions::mint::MintArgs, ID as PROGRAM_ID,
};

const TOKEN_PROGRAM_ID: anchor_lang::prelude::Pubkey = spl_token_2022_interface::ID;
const INITIAL_LAMPORTS: u64 = 10_000_000_000;
const TOKEN_2022_EXTENSION_HEADER_LEN: usize = 4;

struct TestContext {
    svm: LiteSVM,
    payer: Keypair,
}

impl TestContext {
    fn new() -> Self {
        let payer = Keypair::new();
        let program_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/remittance_stablecoin.so");
        let program = fs::read(program_path).expect("run `anchor build` before LiteSVM tests");
        let token_program_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-programs/spl_token_2022.so");
        // LiteSVM's bundled Token-2022 image omits confidential-transfer zk-ops.
        let token_program =
            fs::read(token_program_path).expect("run scripts/build-litesvm-test-programs.sh");
        let mut compute_budget =
            solana_compute_budget::compute_budget::ComputeBudget::new_with_defaults(false, false);
        compute_budget.compute_unit_limit = 1_400_000;
        let mut svm = LiteSVM::new()
            .with_default_programs()
            .with_compute_budget(compute_budget);
        svm.add_program(TOKEN_PROGRAM_ID, &token_program)
            .expect("load Token-2022 with zk-ops enabled");
        svm.add_program(PROGRAM_ID, &program)
            .expect("load Anchor program");
        let proof_program_id =
            spl_token_2022_interface::solana_zk_sdk::zk_elgamal_proof_program::id();
        svm.add_builtin(
            proof_program_id,
            solana_zk_elgamal_proof_program::Entrypoint::vm,
        );
        // Native builtins must be owned by the native loader for the SVM to dispatch them.
        let mut proof_program = svm.get_account(&proof_program_id).unwrap();
        proof_program.owner = solana_sdk_ids::native_loader::id();
        svm.set_account(proof_program_id, proof_program)
            .expect("register native proof program");
        svm.airdrop(&payer.pubkey(), INITIAL_LAMPORTS)
            .expect("fund test payer");
        Self { svm, payer }
    }

    fn send(&mut self, instruction: Instruction, additional_signers: &[&Keypair]) {
        let mut signers = vec![&self.payer];
        signers.extend_from_slice(additional_signers);
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&self.payer.pubkey()),
            &signers,
            self.svm.latest_blockhash(),
        );
        self.svm
            .send_transaction(transaction)
            .expect("transaction should succeed");
    }

    fn send_failing(&mut self, instruction: Instruction, additional_signers: &[&Keypair]) {
        let mut signers = vec![&self.payer];
        signers.extend_from_slice(additional_signers);
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&self.payer.pubkey()),
            &signers,
            self.svm.latest_blockhash(),
        );
        assert!(
            self.svm.send_transaction(transaction).is_err(),
            "transaction should fail"
        );
    }

    fn verify_proof<T, U>(
        &mut self,
        context_address: anchor_lang::prelude::Pubkey,
        proof_instruction: ProofInstruction,
        proof: &T,
    ) where
        T: Pod + ZkProofData<U>,
        U: Pod,
    {
        let context_size = std::mem::size_of::<ProofContextState<U>>();
        let proof_program_id =
            spl_token_2022_interface::solana_zk_sdk::zk_elgamal_proof_program::id();
        self.svm
            .set_account(
                context_address,
                Account {
                    lamports: 10_000_000,
                    data: vec![0; context_size],
                    owner: proof_program_id,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .expect("allocate proof context");
        let payer = self.payer.pubkey();
        let info = ContextStateInfo {
            context_state_account: &context_address,
            context_state_authority: &payer,
        };
        let instruction = proof_instruction.encode_verify_proof(Some(info), proof);
        self.send(instruction, &[]);
    }

    fn instruction(
        &self,
        accounts: Vec<anchor_lang::solana_program::instruction::AccountMeta>,
        data: Vec<u8>,
    ) -> anchor_lang::solana_program::instruction::Instruction {
        anchor_lang::solana_program::instruction::Instruction {
            program_id: PROGRAM_ID,
            accounts,
            data,
        }
    }

    fn initialize_mint(&mut self, mint: &Keypair) -> Instruction {
        self.instruction(
            accounts::InitializeMint {
                issuer: self.payer.pubkey(),
                mint: mint.pubkey(),
                token_program: TOKEN_PROGRAM_ID,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            instruction::InitializeMint {
                args: MintArgs {
                    decimals: 2,
                    transfer_fee_basis_points: 100,
                    maximum_fee: 25,
                    name: "Remit Dollar".into(),
                    symbol: "RUSD".into(),
                    uri: "https://example.invalid/rusd.json".into(),
                },
            }
            .data(),
        )
    }

    fn create_ata(&mut self, mint: &Keypair, owner: &Keypair) -> anchor_lang::prelude::Pubkey {
        let ata = get_associated_token_address_with_program_id(
            &owner.pubkey(),
            &mint.pubkey(),
            &TOKEN_PROGRAM_ID,
        );
        self.send(
            create_associated_token_account(
                &self.payer.pubkey(),
                &owner.pubkey(),
                &mint.pubkey(),
                &TOKEN_PROGRAM_ID,
            ),
            &[],
        );
        ata
    }

    fn allocate_confidential_extensions(
        &mut self,
        token_account: anchor_lang::prelude::Pubkey,
        owner: &Keypair,
    ) {
        let account_data = self.account_data(&token_account);
        let account_state = StateWithExtensions::<TokenAccount>::unpack(&account_data).unwrap();
        let existing_extensions = account_state.get_extension_types().unwrap();
        let required_extensions = [
            ExtensionType::TransferFeeAmount,
            ExtensionType::ConfidentialTransferAccount,
            ExtensionType::ConfidentialTransferFeeAmount,
        ];
        let additional_extensions = required_extensions
            .into_iter()
            .filter(|extension| !existing_extensions.contains(extension))
            .collect::<Vec<_>>();
        if additional_extensions.is_empty() {
            return;
        }
        let reallocate = spl_token_2022_interface::instruction::reallocate(
            &TOKEN_PROGRAM_ID,
            &token_account,
            &self.payer.pubkey(),
            &owner.pubkey(),
            &[],
            &additional_extensions,
        )
        .unwrap();
        self.send(reallocate, &[owner]);
    }

    fn thaw(&mut self, mint: &Keypair, token_account: anchor_lang::prelude::Pubkey) -> Instruction {
        self.instruction(
            accounts::ThawKycAccount {
                mint: mint.pubkey(),
                token_account,
                freeze_authority: self.payer.pubkey(),
                token_program: TOKEN_PROGRAM_ID,
            }
            .to_account_metas(None),
            instruction::ThawKycAccount {}.data(),
        )
    }

    fn account_data(&self, key: &anchor_lang::prelude::Pubkey) -> Vec<u8> {
        self.svm.get_account(key).expect("account exists").data
    }
}

fn encrypted_balance(key: &AeKey, amount: u64) -> Vec<u8> {
    let balance: PodAeCiphertext = key.encrypt(amount).into();
    bytes_of(&balance).to_vec()
}

fn configure_confidential_account(
    context: &mut TestContext,
    mint: &Keypair,
    token_account: anchor_lang::prelude::Pubkey,
    owner: &Keypair,
    elgamal: &ElGamalKeypair,
    aes: &AeKey,
) {
    let proof = PubkeyValidityProofData::new(elgamal).unwrap();
    let proof_context = Keypair::new();
    context.verify_proof(
        proof_context.pubkey(),
        ProofInstruction::VerifyPubkeyValidity,
        &proof,
    );

    // A valid key proof is not sufficient: Token-2022 also requires the token-account owner.
    let unauthorized_owner = Keypair::new();
    let configure = context.instruction(
        accounts::ConfigureConfidentialAccount {
            token_account,
            mint: mint.pubkey(),
            owner: unauthorized_owner.pubkey(),
            pubkey_validity_proof: proof_context.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::ConfigureConfidentialAccount {
            args: remittance_stablecoin::instructions::confidential::ConfigureConfidentialArgs {
                encrypted_zero_balance: encrypted_balance(aes, 0),
                pending_credit_limit: 10,
            },
        }
        .data(),
    );
    context.send_failing(configure, &[&unauthorized_owner]);

    let configure = context.instruction(
        accounts::ConfigureConfidentialAccount {
            token_account,
            mint: mint.pubkey(),
            owner: owner.pubkey(),
            pubkey_validity_proof: proof_context.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::ConfigureConfidentialAccount {
            args: remittance_stablecoin::instructions::confidential::ConfigureConfidentialArgs {
                encrypted_zero_balance: encrypted_balance(aes, 0),
                pending_credit_limit: 10,
            },
        }
        .data(),
    );
    context.send(configure, &[owner]);

    let account_data = context.account_data(&token_account);
    let account_state = StateWithExtensions::<TokenAccount>::unpack(&account_data).unwrap();
    assert!(!bool::from(
        &account_state
            .get_extension::<ConfidentialTransferAccount>()
            .unwrap()
            .approved
    ));

    let approve = context.instruction(
        accounts::ApproveConfidentialAccount {
            token_account,
            mint: mint.pubkey(),
            confidential_authority: context.payer.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::ApproveConfidentialAccount {}.data(),
    );
    context.send(approve, &[]);

    let account_data = context.account_data(&token_account);
    let account_state = StateWithExtensions::<TokenAccount>::unpack(&account_data).unwrap();
    assert!(bool::from(
        &account_state
            .get_extension::<ConfidentialTransferAccount>()
            .unwrap()
            .approved
    ));
}

#[test]
fn mint_extensions_kyc_fee_transfer_and_fee_withdrawal() {
    let mut ctx = TestContext::new();
    let mint = Keypair::new();
    let recipient = Keypair::new();
    let source_owner = Keypair::new();
    let destination_owner = Keypair::new();

    let initialize = ctx.initialize_mint(&mint);
    ctx.send(initialize, &[&mint]);

    let mint_data = ctx.account_data(&mint.pubkey());
    let mint_state = StateWithExtensions::<Mint>::unpack(&mint_data).unwrap();
    let mint_extensions = mint_state.get_extension_types().unwrap();
    for extension in [
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
    ] {
        assert!(
            mint_extensions.contains(&extension),
            "missing {extension:?}"
        );
    }
    assert_eq!(
        mint_state
            .get_extension::<MetadataPointer>()
            .unwrap()
            .metadata_address
            .0,
        mint.pubkey()
    );
    assert_eq!(
        mint_state
            .get_extension::<DefaultAccountState>()
            .unwrap()
            .state,
        u8::from(AccountState::Frozen)
    );
    assert_eq!(
        mint_state
            .get_extension::<MintCloseAuthority>()
            .unwrap()
            .close_authority
            .0,
        ctx.payer.pubkey()
    );
    let metadata = mint_state
        .get_variable_len_extension::<TokenMetadata>()
        .unwrap();
    assert_eq!(metadata.name, "Remit Dollar");
    assert_eq!(metadata.symbol, "RUSD");

    let expected_mint_len = ExtensionType::try_calculate_account_len::<Mint>(&[
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
    ])
    .unwrap();
    assert_eq!(
        mint_data.len(),
        expected_mint_len
            + TOKEN_2022_EXTENSION_HEADER_LEN
            + mint_state
                .get_extension_bytes::<TokenMetadata>()
                .unwrap()
                .len()
    );

    // A third party creates both ATAs; their frozen state still requires issuer KYC thawing.
    let source = ctx.create_ata(&mint, &source_owner);
    let destination = ctx.create_ata(&mint, &destination_owner);
    for account_key in [source, destination] {
        let data = ctx.account_data(&account_key);
        let account_state = StateWithExtensions::<TokenAccount>::unpack(&data).unwrap();
        assert_eq!(account_state.base.state, AccountState::Frozen);
        assert_eq!(account_state.base.mint, mint.pubkey());
    }

    let unauthorized_thaw = ctx.instruction(
        accounts::ThawKycAccount {
            mint: mint.pubkey(),
            token_account: source,
            freeze_authority: source_owner.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::ThawKycAccount {}.data(),
    );
    ctx.send_failing(unauthorized_thaw, &[&source_owner]);
    let thaw_source = ctx.thaw(&mint, source);
    ctx.send(thaw_source, &[]);
    let thaw_destination = ctx.thaw(&mint, destination);
    ctx.send(thaw_destination, &[]);
    let thawed_source_data = ctx.account_data(&source);
    let thawed_source = StateWithExtensions::<TokenAccount>::unpack(&thawed_source_data).unwrap();
    assert_eq!(thawed_source.base.state, AccountState::Initialized);
    let mint_after_thaw_data = ctx.account_data(&mint.pubkey());
    let mint_after_thaw = StateWithExtensions::<Mint>::unpack(&mint_after_thaw_data).unwrap();
    assert_eq!(
        mint_after_thaw
            .get_extension::<DefaultAccountState>()
            .unwrap()
            .state,
        u8::from(AccountState::Frozen)
    );

    let mint_to = spl_token_2022_interface::instruction::mint_to(
        &TOKEN_PROGRAM_ID,
        &mint.pubkey(),
        &source,
        &ctx.payer.pubkey(),
        &[],
        1_000,
    )
    .unwrap();
    ctx.send(mint_to, &[]);
    let mint_state = StateWithExtensions::<Mint>::unpack(&mint_data).unwrap();
    let expected_fee = mint_state
        .get_extension::<TransferFeeConfig>()
        .unwrap()
        .calculate_epoch_fee(
            ctx.svm
                .get_sysvar::<anchor_lang::solana_program::clock::Clock>()
                .epoch,
            100,
        )
        .unwrap();
    assert_eq!(expected_fee, 1);

    let transfer = ctx.instruction(
        accounts::TransferCheckedWithFee {
            source,
            mint: mint.pubkey(),
            destination,
            authority: destination_owner.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::TransferCheckedWithFee { amount: 100 }.data(),
    );
    // The source is owned by source_owner, so a different wallet cannot move it.
    ctx.send_failing(transfer, &[&destination_owner]);

    let transfer = ctx.instruction(
        accounts::TransferCheckedWithFee {
            source,
            mint: mint.pubkey(),
            destination,
            authority: source_owner.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::TransferCheckedWithFee { amount: 100 }.data(),
    );
    ctx.send(transfer, &[&source_owner]);

    let destination_data = ctx.account_data(&destination);
    let destination_state = StateWithExtensions::<TokenAccount>::unpack(&destination_data).unwrap();
    assert_eq!(destination_state.base.amount, 99);
    assert_eq!(
        u64::from(
            destination_state
                .get_extension::<TransferFeeAmount>()
                .unwrap()
                .withheld_amount
        ),
        expected_fee
    );

    let collected_to = ctx.create_ata(&mint, &recipient);
    let thaw_collected_to = ctx.thaw(&mint, collected_to);
    ctx.send(thaw_collected_to, &[]);
    let collect_fees = ctx.instruction(
        accounts::CollectFees {
            mint: mint.pubkey(),
            destination: collected_to,
            withdraw_authority: ctx.payer.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::CollectFees {}.data(),
    );
    let mut collect_fees = collect_fees;
    collect_fees
        .accounts
        .push(anchor_lang::solana_program::instruction::AccountMeta::new(
            destination,
            false,
        ));
    ctx.send(collect_fees, &[]);
    let collected_data = ctx.account_data(&collected_to);
    let collected = StateWithExtensions::<TokenAccount>::unpack(&collected_data).unwrap();
    assert_eq!(collected.base.amount, 1);
}

#[test]
fn mint_close_requires_close_authority_and_zero_supply() {
    let mut ctx = TestContext::new();
    let mint = Keypair::new();
    let initialize = ctx.initialize_mint(&mint);
    ctx.send(initialize, &[&mint]);

    let close_instruction = ctx.instruction(
        accounts::CloseMint {
            mint: mint.pubkey(),
            destination: ctx.payer.pubkey(),
            close_authority: ctx.payer.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::CloseMint {}.data(),
    );
    ctx.send(close_instruction, &[]);
    assert!(ctx.svm.get_account(&mint.pubkey()).is_none());
}

#[test]
fn confidential_reissue_deposit_apply_transfer_and_withdraw() {
    let mut ctx = TestContext::new();
    let mint = Keypair::new();
    let source_owner = Keypair::new();
    let destination_owner = Keypair::new();
    let source_elgamal = ElGamalKeypair::new_rand();
    let destination_elgamal = ElGamalKeypair::new_rand();
    let source_aes = AeKey::new_rand();
    let destination_aes = AeKey::new_rand();
    let fee_elgamal = ElGamalKeypair::new_rand();
    let fee_pubkey: PodElGamalPubkey = (*fee_elgamal.pubkey()).into();
    let confidential_mint_args = remittance_stablecoin::instructions::mint::ConfidentialMintArgs {
        mint: MintArgs {
            decimals: 2,
            transfer_fee_basis_points: 100,
            maximum_fee: 25,
            name: "Remit Dollar".into(),
            symbol: "RUSD".into(),
            uri: "https://example.invalid/rusd.json".into(),
        },
        confidential_fee_elgamal_pubkey: bytes_of(&fee_pubkey).to_vec(),
    };
    let initialize = ctx.instruction(
        accounts::InitializeMint {
            issuer: ctx.payer.pubkey(),
            mint: mint.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        instruction::ReissueConfidentialMint {
            args: confidential_mint_args,
        }
        .data(),
    );
    ctx.send(initialize, &[&mint]);

    let mint_data = ctx.account_data(&mint.pubkey());
    let mint_state = StateWithExtensions::<Mint>::unpack(&mint_data).unwrap();
    let extensions = mint_state.get_extension_types().unwrap();
    for extension in [
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
        ExtensionType::PermanentDelegate,
        ExtensionType::ConfidentialTransferMint,
        ExtensionType::ConfidentialTransferFeeConfig,
    ] {
        assert!(extensions.contains(&extension), "missing {extension:?}");
    }
    let confidential_fixed_extensions = [
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
        ExtensionType::PermanentDelegate,
        ExtensionType::ConfidentialTransferMint,
        ExtensionType::ConfidentialTransferFeeConfig,
    ];
    assert_eq!(
        mint_data.len(),
        ExtensionType::try_calculate_account_len::<Mint>(&confidential_fixed_extensions).unwrap()
            + TOKEN_2022_EXTENSION_HEADER_LEN
            + mint_state
                .get_extension_bytes::<TokenMetadata>()
                .unwrap()
                .len()
    );
    assert_eq!(
        mint_state
            .get_extension::<PermanentDelegate>()
            .unwrap()
            .delegate
            .0,
        ctx.payer.pubkey()
    );
    assert!(!bool::from(
        &mint_state
            .get_extension::<ConfidentialTransferMint>()
            .unwrap()
            .auto_approve_new_accounts
    ));

    let source = ctx.create_ata(&mint, &source_owner);
    let destination = ctx.create_ata(&mint, &destination_owner);
    ctx.allocate_confidential_extensions(source, &source_owner);
    ctx.allocate_confidential_extensions(destination, &destination_owner);
    let thaw_source = ctx.thaw(&mint, source);
    ctx.send(thaw_source, &[]);
    let thaw_destination = ctx.thaw(&mint, destination);
    ctx.send(thaw_destination, &[]);
    configure_confidential_account(
        &mut ctx,
        &mint,
        source,
        &source_owner,
        &source_elgamal,
        &source_aes,
    );
    configure_confidential_account(
        &mut ctx,
        &mint,
        destination,
        &destination_owner,
        &destination_elgamal,
        &destination_aes,
    );

    let mint_to = spl_token_2022_interface::instruction::mint_to(
        &TOKEN_PROGRAM_ID,
        &mint.pubkey(),
        &source,
        &ctx.payer.pubkey(),
        &[],
        2_000,
    )
    .unwrap();
    ctx.send(mint_to, &[]);
    let deposit = ctx.instruction(
        accounts::ConfidentialAccountOwner {
            token_account: source,
            mint: mint.pubkey(),
            owner: source_owner.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::DepositConfidential { amount: 2_000 }.data(),
    );
    ctx.send(deposit, &[&source_owner]);

    let source_data = ctx.account_data(&source);
    let source_state = StateWithExtensions::<TokenAccount>::unpack(&source_data).unwrap();
    let pending_credits = u64::from(
        source_state
            .get_extension::<ConfidentialTransferAccount>()
            .unwrap()
            .pending_balance_credit_counter,
    );
    assert_eq!(pending_credits, 1);
    let apply = ctx.instruction(
        accounts::ConfidentialAccountOwner {
            token_account: source,
            mint: mint.pubkey(),
            owner: source_owner.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::ApplyPendingBalance {
            args: remittance_stablecoin::instructions::confidential::ApplyPendingArgs {
                pending_credits,
                new_decryptable_balance: encrypted_balance(&source_aes, 2_000),
            },
        }
        .data(),
    );
    ctx.send(apply, &[&source_owner]);

    let source_data = ctx.account_data(&source);
    let source_state = StateWithExtensions::<TokenAccount>::unpack(&source_data).unwrap();
    let source_confidential = source_state
        .get_extension::<ConfidentialTransferAccount>()
        .unwrap();
    let source_available: ElGamalCiphertext =
        source_confidential.available_balance.try_into().unwrap();
    let source_decryptable: AeCiphertext = source_confidential
        .decryptable_available_balance
        .try_into()
        .unwrap();
    assert_eq!(source_decryptable.decrypt(&source_aes).unwrap(), 2_000);

    let destination_data = ctx.account_data(&destination);
    let destination_state = StateWithExtensions::<TokenAccount>::unpack(&destination_data).unwrap();
    let destination_confidential = destination_state
        .get_extension::<ConfidentialTransferAccount>()
        .unwrap();
    let destination_elgamal_pubkey: ElGamalPubkey =
        destination_confidential.elgamal_pubkey.try_into().unwrap();
    let proofs = transfer_with_fee_split_proof_data(
        &source_available,
        &source_decryptable,
        100,
        &source_elgamal,
        &source_aes,
        &destination_elgamal_pubkey,
        None,
        fee_elgamal.pubkey(),
        100,
        25,
    )
    .unwrap();

    let equality_context = Keypair::new();
    ctx.verify_proof(
        equality_context.pubkey(),
        ProofInstruction::VerifyCiphertextCommitmentEquality,
        &proofs.equality_proof_data,
    );
    let transfer_validity_context = Keypair::new();
    ctx.verify_proof(
        transfer_validity_context.pubkey(),
        ProofInstruction::VerifyBatchedGroupedCiphertext3HandlesValidity,
        &proofs
            .transfer_amount_ciphertext_validity_proof_data_with_ciphertext
            .proof_data,
    );
    let fee_sigma_context = Keypair::new();
    ctx.verify_proof(
        fee_sigma_context.pubkey(),
        ProofInstruction::VerifyPercentageWithCap,
        &proofs.percentage_with_cap_proof_data,
    );
    let fee_ciphertext_context = Keypair::new();
    ctx.verify_proof(
        fee_ciphertext_context.pubkey(),
        ProofInstruction::VerifyBatchedGroupedCiphertext2HandlesValidity,
        &proofs.fee_ciphertext_validity_proof_data,
    );
    let range_context = Keypair::new();
    ctx.verify_proof(
        range_context.pubkey(),
        ProofInstruction::VerifyBatchedRangeProofU256,
        &proofs.range_proof_data,
    );

    let auditor_ciphertext_lo = proofs
        .transfer_amount_ciphertext_validity_proof_data_with_ciphertext
        .ciphertext_lo;
    let auditor_ciphertext_hi = proofs
        .transfer_amount_ciphertext_validity_proof_data_with_ciphertext
        .ciphertext_hi;
    let transfer = ctx.instruction(
        accounts::ConfidentialTransfer {
            source,
            mint: mint.pubkey(),
            destination,
            owner: source_owner.pubkey(),
            equality_proof: equality_context.pubkey(),
            transfer_validity_proof: transfer_validity_context.pubkey(),
            fee_sigma_proof: fee_sigma_context.pubkey(),
            fee_ciphertext_validity_proof: fee_ciphertext_context.pubkey(),
            range_proof: range_context.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::ConfidentialTransfer {
            args: remittance_stablecoin::instructions::confidential::ConfidentialTransferArgs {
                new_source_decryptable_balance: encrypted_balance(&source_aes, 1_900),
                auditor_ciphertext_lo: bytes_of(&auditor_ciphertext_lo).to_vec(),
                auditor_ciphertext_hi: bytes_of(&auditor_ciphertext_hi).to_vec(),
            },
        }
        .data(),
    );
    ctx.send(transfer, &[&source_owner]);

    let destination_data = ctx.account_data(&destination);
    let destination_state = StateWithExtensions::<TokenAccount>::unpack(&destination_data).unwrap();
    let destination_confidential = destination_state
        .get_extension::<ConfidentialTransferAccount>()
        .unwrap();
    let incoming_credits = u64::from(destination_confidential.pending_balance_credit_counter);
    assert_eq!(incoming_credits, 1);
    let destination_available: ElGamalCiphertext = destination_confidential
        .available_balance
        .try_into()
        .unwrap();
    let pending_lo: ElGamalCiphertext = destination_confidential
        .pending_balance_lo
        .try_into()
        .unwrap();
    let pending_hi: ElGamalCiphertext = destination_confidential
        .pending_balance_hi
        .try_into()
        .unwrap();
    let pending_amount = try_combine_lo_hi_ciphertexts(&pending_lo, &pending_hi, 16).unwrap();
    let post_apply_balance = destination_available + pending_amount;
    let withdraw_proofs =
        withdraw_proof_data(&post_apply_balance, 99, 99, &destination_elgamal).unwrap();
    let withdraw_equality_context = Keypair::new();
    ctx.verify_proof(
        withdraw_equality_context.pubkey(),
        ProofInstruction::VerifyCiphertextCommitmentEquality,
        &withdraw_proofs.equality_proof_data,
    );
    let withdraw_range_context = Keypair::new();
    ctx.verify_proof(
        withdraw_range_context.pubkey(),
        ProofInstruction::VerifyBatchedRangeProofU64,
        &withdraw_proofs.range_proof_data,
    );
    let withdraw = ctx.instruction(
        accounts::ConfidentialWithdraw {
            token_account: destination,
            mint: mint.pubkey(),
            owner: destination_owner.pubkey(),
            equality_proof: withdraw_equality_context.pubkey(),
            range_proof: withdraw_range_context.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::WithdrawConfidential {
            args: remittance_stablecoin::instructions::confidential::ConfidentialWithdrawArgs {
                pending_credits: incoming_credits,
                balance_after_apply: encrypted_balance(&destination_aes, 99),
                amount: 99,
                balance_after_withdraw: encrypted_balance(&destination_aes, 0),
            },
        }
        .data(),
    );
    ctx.send(withdraw, &[&destination_owner]);

    let destination_data = ctx.account_data(&destination);
    let destination_state = StateWithExtensions::<TokenAccount>::unpack(&destination_data).unwrap();
    assert_eq!(destination_state.base.amount, 99);
    let destination_confidential = destination_state
        .get_extension::<ConfidentialTransferAccount>()
        .unwrap();
    let remaining_decrypted: AeCiphertext = destination_confidential
        .decryptable_available_balance
        .try_into()
        .unwrap();
    assert_eq!(remaining_decrypted.decrypt(&destination_aes).unwrap(), 0);

    // The permanent delegate can seize public tokens, but not the confidential balance.
    let mint_to = spl_token_2022_interface::instruction::mint_to(
        &TOKEN_PROGRAM_ID,
        &mint.pubkey(),
        &source,
        &ctx.payer.pubkey(),
        &[],
        100,
    )
    .unwrap();
    ctx.send(mint_to, &[]);
    let seize = ctx.instruction(
        accounts::SeizePublicBalance {
            source,
            mint: mint.pubkey(),
            destination,
            permanent_delegate: ctx.payer.pubkey(),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        instruction::SeizePublicBalance { amount: 50 }.data(),
    );
    ctx.send(seize, &[]);
    let source_data = ctx.account_data(&source);
    let source_state = StateWithExtensions::<TokenAccount>::unpack(&source_data).unwrap();
    assert_eq!(source_state.base.amount, 50);
    let destination_data = ctx.account_data(&destination);
    let destination_state = StateWithExtensions::<TokenAccount>::unpack(&destination_data).unwrap();
    assert_eq!(destination_state.base.amount, 148);
}
