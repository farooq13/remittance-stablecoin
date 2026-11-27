use {
    anchor_lang::{
        prelude::Pubkey,
        solana_program::{instruction::Instruction, program_pack::Pack, system_instruction},
        InstructionData, ToAccountMetas,
    },
    litesvm::LiteSVM,
    solana_keypair::Keypair,
    solana_signer::Signer,
    solana_transaction::Transaction,
    spl_token_2022_interface::{
        extension::cpi_guard::instruction as cpi_guard_instruction,
        extension::{
            cpi_guard::CpiGuard, BaseStateWithExtensions, ExtensionType, StateWithExtensions,
        },
        instruction as token_instruction,
        state::{Account as TokenAccountState, Mint},
    },
    std::{fs, path::PathBuf},
};

use remittance_agent::{accounts, instruction, ID as PROGRAM_ID};

const TOKEN_PROGRAM_ID: Pubkey = spl_token_2022_interface::ID;
const INITIAL_LAMPORTS: u64 = 10_000_000_000;

struct TestContext {
    svm: LiteSVM,
    payer: Keypair,
}

impl TestContext {
    fn new() -> Self {
        let payer = Keypair::new();
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let program = fs::read(manifest_dir.join("../../target/deploy/remittance_agent.so"))
            .expect("run `anchor build` before LiteSVM tests");
        let token_program =
            fs::read(manifest_dir.join("../../target/test-programs/spl_token_2022.so"))
                .expect("run scripts/build-litesvm-test-programs.sh");

        let mut svm = LiteSVM::new().with_default_programs();
        svm.add_program(TOKEN_PROGRAM_ID, &token_program)
            .expect("load Token-2022");
        svm.add_program(PROGRAM_ID, &program)
            .expect("load remittance-agent");
        svm.airdrop(&payer.pubkey(), INITIAL_LAMPORTS)
            .expect("fund test payer");
        Self { svm, payer }
    }

    fn send(&mut self, instruction: Instruction, additional_signers: &[&Keypair]) {
        self.svm.expire_blockhash();
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

    fn send_failing(&mut self, instruction: Instruction) {
        self.svm.expire_blockhash();
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&self.payer.pubkey()),
            &[&self.payer],
            self.svm.latest_blockhash(),
        );
        assert!(
            self.svm.send_transaction(transaction).is_err(),
            "transaction should fail"
        );
    }

    fn create_account(&mut self, keypair: &Keypair, space: usize) {
        let rent = self.svm.minimum_balance_for_rent_exemption(space);
        self.send(
            system_instruction::create_account(
                &self.payer.pubkey(),
                &keypair.pubkey(),
                rent,
                space as u64,
                &TOKEN_PROGRAM_ID,
            ),
            &[keypair],
        );
    }

    fn create_mint(&mut self) -> Keypair {
        let mint = Keypair::new();
        self.create_account(&mint, Mint::LEN);
        self.send(
            token_instruction::initialize_mint2(
                &TOKEN_PROGRAM_ID,
                &mint.pubkey(),
                &self.payer.pubkey(),
                None,
                2,
            )
            .unwrap(),
            &[],
        );
        mint
    }

    fn create_token_account(&mut self, mint: &Pubkey, owner: &Pubkey) -> Keypair {
        let account = Keypair::new();
        self.create_account(&account, TokenAccountState::LEN);
        self.send(
            token_instruction::initialize_account3(
                &TOKEN_PROGRAM_ID,
                &account.pubkey(),
                mint,
                owner,
            )
            .unwrap(),
            &[],
        );
        account
    }

    fn mint_to(&mut self, mint: &Pubkey, destination: &Pubkey, amount: u64) {
        self.send(
            token_instruction::mint_to(
                &TOKEN_PROGRAM_ID,
                mint,
                destination,
                &self.payer.pubkey(),
                &[],
                amount,
            )
            .unwrap(),
            &[],
        );
    }

    fn approve_delegate(
        &mut self,
        source: &Pubkey,
        mint: &Pubkey,
        delegate: &Pubkey,
        owner: &Keypair,
        allowance: u64,
    ) {
        self.send(
            token_instruction::approve_checked(
                &TOKEN_PROGRAM_ID,
                source,
                mint,
                delegate,
                &owner.pubkey(),
                &[],
                allowance,
                2,
            )
            .unwrap(),
            &[owner],
        );
    }

    fn account_data(&self, address: &Pubkey) -> Vec<u8> {
        self.svm.get_account(address).expect("account exists").data
    }

    fn token_account(&self, address: &Pubkey) -> TokenAccountState {
        let data = self.account_data(address);
        StateWithExtensions::<TokenAccountState>::unpack(&data)
            .unwrap()
            .base
    }

    fn execute_transfer(
        &self,
        source: Pubkey,
        destination: Pubkey,
        mint: Pubkey,
        owner: Pubkey,
        delegate: Pubkey,
        amount: u64,
    ) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::ExecuteTransfer {
                source,
                destination,
                mint,
                delegate,
                owner,
                token_program: TOKEN_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::ExecuteTransfer { amount }.data(),
        }
    }
}

struct DelegatedAccounts {
    owner: Keypair,
    mint: Keypair,
    source: Keypair,
    destination: Keypair,
    delegate: Pubkey,
}

impl DelegatedAccounts {
    fn new(ctx: &mut TestContext, allowance: u64) -> Self {
        let owner = Keypair::new();
        let recipient_owner = Keypair::new();
        let mint = ctx.create_mint();
        let source = ctx.create_token_account(&mint.pubkey(), &owner.pubkey());
        let destination = ctx.create_token_account(&mint.pubkey(), &recipient_owner.pubkey());
        ctx.mint_to(&mint.pubkey(), &source.pubkey(), 1_000);

        let (delegate, _) = remittance_agent::delegate_address(owner.pubkey());
        ctx.approve_delegate(
            &source.pubkey(),
            &mint.pubkey(),
            &delegate,
            &owner,
            allowance,
        );

        Self {
            owner,
            mint,
            source,
            destination,
            delegate,
        }
    }

    fn transfer_instruction(&self, ctx: &TestContext, amount: u64) -> Instruction {
        ctx.execute_transfer(
            self.source.pubkey(),
            self.destination.pubkey(),
            self.mint.pubkey(),
            self.owner.pubkey(),
            self.delegate,
            amount,
        )
    }
}

#[test]
fn approved_delegate_transfer_survives_cpi_guard_without_instruction_changes() {
    let mut ctx = TestContext::new();
    let accounts = DelegatedAccounts::new(&mut ctx, 500);
    let transfer = accounts.transfer_instruction(&ctx, 100);
    assert_ne!(ctx.payer.pubkey(), accounts.owner.pubkey());
    ctx.send(transfer.clone(), &[]);

    ctx.send(
        token_instruction::reallocate(
            &TOKEN_PROGRAM_ID,
            &accounts.source.pubkey(),
            &ctx.payer.pubkey(),
            &accounts.owner.pubkey(),
            &[],
            &[ExtensionType::CpiGuard],
        )
        .unwrap(),
        &[&accounts.owner],
    );
    ctx.send(
        cpi_guard_instruction::enable_cpi_guard(
            &TOKEN_PROGRAM_ID,
            &accounts.source.pubkey(),
            &accounts.owner.pubkey(),
            &[],
        )
        .unwrap(),
        &[&accounts.owner],
    );

    let guarded_source = ctx.account_data(&accounts.source.pubkey());
    let guarded_state = StateWithExtensions::<TokenAccountState>::unpack(&guarded_source).unwrap();
    assert!(bool::from(
        &guarded_state.get_extension::<CpiGuard>().unwrap().lock_cpi
    ));

    ctx.send(transfer, &[]);

    let source_state = ctx.token_account(&accounts.source.pubkey());
    assert_eq!(source_state.amount, 800);
    assert_eq!(source_state.delegated_amount, 300);
    assert_eq!(
        source_state.delegate.unwrap(),
        accounts.delegate,
        "the approved PDA remains the delegate after CPI Guard is enabled"
    );

    let destination_state = ctx.token_account(&accounts.destination.pubkey());
    assert_eq!(destination_state.amount, 200);
}

#[test]
fn rejects_transfer_when_delegate_pda_does_not_match_owner() {
    let mut ctx = TestContext::new();
    let accounts = DelegatedAccounts::new(&mut ctx, 500);
    let wrong_delegate = Keypair::new();
    let instruction = ctx.execute_transfer(
        accounts.source.pubkey(),
        accounts.destination.pubkey(),
        accounts.mint.pubkey(),
        accounts.owner.pubkey(),
        wrong_delegate.pubkey(),
        100,
    );

    ctx.send_failing(instruction);

    assert_eq!(ctx.token_account(&accounts.source.pubkey()).amount, 1_000);
    assert_eq!(ctx.token_account(&accounts.destination.pubkey()).amount, 0);
    assert_eq!(
        ctx.token_account(&accounts.source.pubkey())
            .delegated_amount,
        500
    );
}

#[test]
fn rejects_transfer_when_supplied_owner_is_not_source_owner() {
    let mut ctx = TestContext::new();
    let accounts = DelegatedAccounts::new(&mut ctx, 500);
    let wrong_owner = Keypair::new();
    let (wrong_delegate, _) = remittance_agent::delegate_address(wrong_owner.pubkey());
    let instruction = ctx.execute_transfer(
        accounts.source.pubkey(),
        accounts.destination.pubkey(),
        accounts.mint.pubkey(),
        wrong_owner.pubkey(),
        wrong_delegate,
        100,
    );

    ctx.send_failing(instruction);

    assert_eq!(ctx.token_account(&accounts.source.pubkey()).amount, 1_000);
    assert_eq!(ctx.token_account(&accounts.destination.pubkey()).amount, 0);
}

#[test]
fn rejects_transfer_to_destination_for_different_mint() {
    let mut ctx = TestContext::new();
    let accounts = DelegatedAccounts::new(&mut ctx, 500);
    let wrong_mint = ctx.create_mint();
    let wrong_destination =
        ctx.create_token_account(&wrong_mint.pubkey(), &Keypair::new().pubkey());
    let instruction = ctx.execute_transfer(
        accounts.source.pubkey(),
        wrong_destination.pubkey(),
        accounts.mint.pubkey(),
        accounts.owner.pubkey(),
        accounts.delegate,
        100,
    );

    ctx.send_failing(instruction);

    assert_eq!(ctx.token_account(&accounts.source.pubkey()).amount, 1_000);
    assert_eq!(ctx.token_account(&wrong_destination.pubkey()).amount, 0);
    assert_eq!(
        ctx.token_account(&accounts.source.pubkey())
            .delegated_amount,
        500
    );
}

#[test]
fn rejects_transfer_above_remaining_allowance_without_changing_balances() {
    let mut ctx = TestContext::new();
    let accounts = DelegatedAccounts::new(&mut ctx, 150);
    ctx.send(accounts.transfer_instruction(&ctx, 100), &[]);

    ctx.send_failing(accounts.transfer_instruction(&ctx, 51));

    let source = ctx.token_account(&accounts.source.pubkey());
    let destination = ctx.token_account(&accounts.destination.pubkey());
    assert_eq!(source.amount, 900);
    assert_eq!(destination.amount, 100);
    assert_eq!(source.delegated_amount, 50);
}
