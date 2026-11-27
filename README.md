# Remittance Stablecoin

A Solana workspace for experimenting with compliance-aware stablecoin
operations and delegated transfer automation. The repository contains two
separate Anchor programs: one manages the Token-2022 remittance mint and its
account operations; the other executes transfers using a user's explicitly
approved delegate allowance.

Each program has its own documentation:

- [Remittance Stablecoin program](programs/remittance-stablecoin/README.md)
- [Remittance Agent program](programs/remittance-agent/README.md)

## Programs

### `remittance-stablecoin`

Issues and manages a Token-2022 mint with transfer fees, on-mint metadata, and
frozen-by-default token accounts. It also provides public transfers, fee
collection, KYC thawing, confidential-transfer operations, and a permanent
delegate path for moving **public** balances.

**Program ID:** `E8js5UVXNrdYmGeCCBKE2vdL1qK4XbjkaZ88TJDi19Wz`

### `remittance-agent`

Executes checked Token-2022 transfers using a PDA that the token-account owner
has approved as a delegate. Token-2022 enforces the approved allowance. The
LiteSVM tests also verify that this delegated transfer path works after CPI
Guard is enabled on the source account.

**Program ID:** `2RxSzAKT8HVc3ynLrYPJh4PJVoYC9PsStGkMrBiutgAD`

## Important limitations

- The agent does not manage approvals, set a recipient policy, or maintain a
  separate allowance. The approved delegate can transfer to any destination
  account for the same mint, up to the remaining Token-2022 allowance. The
  owner can revoke the approval through Token-2022.
- The stablecoin's permanent-delegate seizure path only transfers public
  balances. The current programs do not provide a confidential-balance seizure
  or recovery path. See the [agent README's compliance limitation](programs/remittance-agent/README.md#compliance-limitation-confidential-balances).
- Creating a confidential replacement mint does not migrate supply or balances
  from an existing mint; migration requires holder-authorized operations.
- These programs provide on-chain instructions, not a client application,
  custody service, KYC provider, or production compliance process. Review the
  program-specific documentation and assess operational and regulatory
  requirements before use.

## Repository layout

```text
.
├── Anchor.toml
├── Cargo.toml
├── programs/
│   ├── remittance-agent/
│   │   ├── README.md
│   │   ├── src/lib.rs
│   │   └── tests/delegated_transfer.rs
│   └── remittance-stablecoin/
│       ├── README.md
│       ├── src/lib.rs
│       └── tests/litesvm_lifecycle.rs
└── scripts/
    └── build-litesvm-test-programs.sh
```

Both programs are members of the Cargo workspace. Anchor configuration and
program IDs are in `Anchor.toml`.

## Build and test

The configured Anchor test command builds the programs and the Token-2022 SBF
test program required by the LiteSVM confidential-transfer tests, then runs the
workspace Rust tests:

```sh
anchor test
```

To run a package's Rust tests after its SBF artifacts and the Token-2022 test
program have been built:

```sh
cargo test -p remittance-stablecoin
cargo test -p remittance-agent
```

The workspace targets Rust 1.89.0; see [`rust-toolchain.toml`](rust-toolchain.toml)
for the pinned toolchain configuration.
