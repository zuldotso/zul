//! ZUL settlement + bridge program (Solana L1).
//!
//! Stage-0 optimistic rollup: the sequencer posts batch records (state root
//! + DA hash) and users bridge SOL in/out. Withdrawals are claimed by
//! proving a withdrawal commitment is in a posted state root via the same
//! blake3 SMT the L2 maintains (see `smt`).
//!
//! v1 trust model (see docs/PLAN.md §5): the posted state roots are not yet
//! challenged on-chain by re-execution; the challenge window is procedural.

use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{
    self, Mint, TokenAccount, TokenInterface, TransferChecked,
};

pub mod smt;

declare_id!("2hwZWDpMYAMVZrmMjjHuFK4jbMrTJq62fL9d5cPPadHh");

const CONFIG_SEED: &[u8] = b"config";
const VAULT_SEED: &[u8] = b"vault";
const BATCH_SEED: &[u8] = b"batch";
const CLAIM_SEED: &[u8] = b"claim";

#[program]
pub mod zul_settlement {
    use super::*;

    /// One-time setup: the signer becomes the sequencer authority.
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.authority = ctx.accounts.authority.key();
        config.batch_count = 0;
        config.total_deposited = 0;
        config.bump = ctx.bumps.config;
        config.vault_bump = ctx.bumps.vault;
        Ok(())
    }

    /// Post the next settlement batch. Batches are strictly sequential.
    pub fn post_batch(
        ctx: Context<PostBatch>,
        batch_no: u64,
        first_slot: u64,
        last_slot: u64,
        state_root: [u8; 32],
        withdrawals_root: [u8; 32],
        da_hash: [u8; 32],
        chunk_count: u32,
    ) -> Result<()> {
        let config = &mut ctx.accounts.config;
        require_keys_eq!(
            ctx.accounts.authority.key(),
            config.authority,
            SettlementError::Unauthorized
        );
        require_eq!(batch_no, config.batch_count, SettlementError::NonSequentialBatch);
        require!(last_slot >= first_slot, SettlementError::InvalidSlotRange);

        let batch = &mut ctx.accounts.batch;
        batch.batch_no = batch_no;
        batch.first_slot = first_slot;
        batch.last_slot = last_slot;
        batch.state_root = state_root;
        batch.withdrawals_root = withdrawals_root;
        batch.da_hash = da_hash;
        batch.chunk_count = chunk_count;
        batch.posted_at = Clock::get()?.unix_timestamp;

        config.batch_count = config
            .batch_count
            .checked_add(1)
            .ok_or(SettlementError::Overflow)?;

        emit!(BatchPosted {
            batch_no,
            first_slot,
            last_slot,
            state_root,
            da_hash,
        });
        Ok(())
    }

    /// Deposit SOL into the bridge vault, crediting `l2_recipient` on the L2.
    pub fn deposit_sol(ctx: Context<DepositSol>, amount: u64, l2_recipient: Pubkey) -> Result<()> {
        require!(amount > 0, SettlementError::ZeroAmount);

        let ix = anchor_lang::solana_program::system_instruction::transfer(
            &ctx.accounts.depositor.key(),
            &ctx.accounts.vault.key(),
            amount,
        );
        anchor_lang::solana_program::program::invoke(
            &ix,
            &[
                ctx.accounts.depositor.to_account_info(),
                ctx.accounts.vault.to_account_info(),
            ],
        )?;

        let config = &mut ctx.accounts.config;
        config.total_deposited = config
            .total_deposited
            .checked_add(amount)
            .ok_or(SettlementError::Overflow)?;

        emit!(Deposited {
            depositor: ctx.accounts.depositor.key(),
            amount,
            l2_recipient,
            mint: None,
            decimals: 9,
        });
        Ok(())
    }

    /// Deposit `amount` of an SPL token into the bridge's per-mint vault ATA,
    /// crediting `l2_recipient` with the wrapped token on the L2. The L2 mints a
    /// deterministic wrapped representation of `mint` (the deposit watcher reads
    /// the `Deposited` event's `mint` + `decimals`). Works for both Token and
    /// Token-2022 mints via the token interface.
    pub fn deposit_spl(ctx: Context<DepositSpl>, amount: u64, l2_recipient: Pubkey) -> Result<()> {
        require!(amount > 0, SettlementError::ZeroAmount);

        let decimals = ctx.accounts.mint.decimals;
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.depositor_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.vault_token_account.to_account_info(),
                    authority: ctx.accounts.depositor.to_account_info(),
                },
            ),
            amount,
            decimals,
        )?;

        emit!(Deposited {
            depositor: ctx.accounts.depositor.key(),
            amount,
            l2_recipient,
            mint: Some(ctx.accounts.mint.key()),
            decimals,
        });
        Ok(())
    }

    /// Claim an SPL withdrawal: prove the asset-bound commitment is in a posted
    /// batch's withdrawals root, then release the tokens from the per-mint vault
    /// ATA to the recipient. The `claim` PDA prevents double claims.
    pub fn claim_withdrawal_spl(
        ctx: Context<ClaimWithdrawalSpl>,
        _batch_no: u64,
        amount: u64,
        nonce: u64,
        bitmap: [u8; 32],
        siblings: Vec<[u8; 32]>,
    ) -> Result<()> {
        require!(amount > 0, SettlementError::ZeroAmount);

        let recipient = ctx.accounts.recipient.key().to_bytes();
        // SPL withdrawals bind the L1 mint as the asset id.
        let asset_id = ctx.accounts.mint.key().to_bytes();
        let leaf_pubkey = smt::withdrawal_pubkey(&recipient, nonce);
        let account_hash = smt::withdrawal_account_hash(&recipient, &asset_id, amount, nonce);
        let proof = smt::SmtProof {
            bitmap,
            siblings: &siblings,
        };
        require!(
            smt::verify(
                &ctx.accounts.batch.withdrawals_root,
                &leaf_pubkey,
                &account_hash,
                &proof,
            ),
            SettlementError::InvalidProof
        );

        let decimals = ctx.accounts.mint.decimals;
        let vault_bump = ctx.accounts.config.vault_bump;
        let seeds: &[&[u8]] = &[VAULT_SEED, &[vault_bump]];
        let signer = &[seeds];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.vault_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.recipient_token_account.to_account_info(),
                    authority: ctx.accounts.vault.to_account_info(),
                },
                signer,
            ),
            amount,
            decimals,
        )?;

        let claim = &mut ctx.accounts.claim;
        claim.recipient = ctx.accounts.recipient.key();
        claim.amount = amount;
        claim.nonce = nonce;
        claim.claimed_at = Clock::get()?.unix_timestamp;

        emit!(Withdrawn {
            recipient: ctx.accounts.recipient.key(),
            amount,
            nonce,
        });
        Ok(())
    }

    /// Claim a withdrawal by proving its commitment is in a posted batch's
    /// state root. The `claim` PDA's existence prevents double claims.
    pub fn claim_withdrawal(
        ctx: Context<ClaimWithdrawal>,
        _batch_no: u64,
        amount: u64,
        nonce: u64,
        bitmap: [u8; 32],
        siblings: Vec<[u8; 32]>,
    ) -> Result<()> {
        require!(amount > 0, SettlementError::ZeroAmount);

        let recipient = ctx.accounts.recipient.key().to_bytes();
        let leaf_pubkey = smt::withdrawal_pubkey(&recipient, nonce);
        // Native ZUL/SOL withdrawal: asset id is the reserved all-zero value.
        // SPL withdrawals are claimed via `claim_withdrawal_spl`.
        let account_hash = smt::withdrawal_account_hash(&recipient, &[0u8; 32], amount, nonce);
        let proof = smt::SmtProof {
            bitmap,
            siblings: &siblings,
        };
        require!(
            smt::verify(
                &ctx.accounts.batch.withdrawals_root,
                &leaf_pubkey,
                &account_hash,
                &proof,
            ),
            SettlementError::InvalidProof
        );

        // Pay out from the vault PDA.
        let vault_bump = ctx.accounts.config.vault_bump;
        let seeds: &[&[u8]] = &[VAULT_SEED, &[vault_bump]];
        let signer = &[seeds];
        let ix = anchor_lang::solana_program::system_instruction::transfer(
            &ctx.accounts.vault.key(),
            &ctx.accounts.recipient.key(),
            amount,
        );
        anchor_lang::solana_program::program::invoke_signed(
            &ix,
            &[
                ctx.accounts.vault.to_account_info(),
                ctx.accounts.recipient.to_account_info(),
            ],
            signer,
        )?;

        let claim = &mut ctx.accounts.claim;
        claim.recipient = ctx.accounts.recipient.key();
        claim.amount = amount;
        claim.nonce = nonce;
        claim.claimed_at = Clock::get()?.unix_timestamp;

        emit!(Withdrawn {
            recipient: ctx.accounts.recipient.key(),
            amount,
            nonce,
        });
        Ok(())
    }
}

// ----- accounts -----------------------------------------------------------

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Config::SIZE,
        seeds = [CONFIG_SEED],
        bump
    )]
    pub config: Account<'info, Config>,
    /// CHECK: system-owned vault PDA; only holds lamports.
    #[account(seeds = [VAULT_SEED], bump)]
    pub vault: UncheckedAccount<'info>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(batch_no: u64)]
pub struct PostBatch<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        init,
        payer = authority,
        space = 8 + Batch::SIZE,
        seeds = [BATCH_SEED, &batch_no.to_le_bytes()],
        bump
    )]
    pub batch: Account<'info, Batch>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DepositSol<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: system-owned vault PDA.
    #[account(mut, seeds = [VAULT_SEED], bump = config.vault_bump)]
    pub vault: UncheckedAccount<'info>,
    #[account(mut)]
    pub depositor: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DepositSpl<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: the vault PDA is only the ATA authority here; it holds no data.
    #[account(seeds = [VAULT_SEED], bump = config.vault_bump)]
    pub vault: UncheckedAccount<'info>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = depositor,
    )]
    pub depositor_token_account: InterfaceAccount<'info, TokenAccount>,
    /// Per-mint vault ATA, created the first time this mint is bridged.
    #[account(
        init_if_needed,
        payer = depositor,
        associated_token::mint = mint,
        associated_token::authority = vault,
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub depositor: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(batch_no: u64, amount: u64, nonce: u64)]
pub struct ClaimWithdrawalSpl<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(seeds = [BATCH_SEED, &batch_no.to_le_bytes()], bump)]
    pub batch: Account<'info, Batch>,
    /// CHECK: vault PDA; signs the token transfer as the ATA authority.
    #[account(seeds = [VAULT_SEED], bump = config.vault_bump)]
    pub vault: UncheckedAccount<'info>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = vault,
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    /// CHECK: token destination owner; bound into the proof, so any submitter is fine.
    pub recipient: UncheckedAccount<'info>,
    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = mint,
        associated_token::authority = recipient,
    )]
    pub recipient_token_account: InterfaceAccount<'info, TokenAccount>,
    #[account(
        init,
        payer = payer,
        space = 8 + WithdrawalClaim::SIZE,
        seeds = [CLAIM_SEED, recipient.key().as_ref(), &nonce.to_le_bytes()],
        bump
    )]
    pub claim: Account<'info, WithdrawalClaim>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(batch_no: u64, amount: u64, nonce: u64)]
pub struct ClaimWithdrawal<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        seeds = [BATCH_SEED, &batch_no.to_le_bytes()],
        bump
    )]
    pub batch: Account<'info, Batch>,
    /// CHECK: system-owned vault PDA.
    #[account(mut, seeds = [VAULT_SEED], bump = config.vault_bump)]
    pub vault: UncheckedAccount<'info>,
    /// CHECK: paid out to; bound into the proof, so any submitter is fine.
    #[account(mut)]
    pub recipient: UncheckedAccount<'info>,
    #[account(
        init,
        payer = payer,
        space = 8 + WithdrawalClaim::SIZE,
        seeds = [CLAIM_SEED, recipient.key().as_ref(), &nonce.to_le_bytes()],
        bump
    )]
    pub claim: Account<'info, WithdrawalClaim>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

// ----- state --------------------------------------------------------------

#[account]
pub struct Config {
    pub authority: Pubkey,
    pub batch_count: u64,
    pub total_deposited: u64,
    pub bump: u8,
    pub vault_bump: u8,
}
impl Config {
    pub const SIZE: usize = 32 + 8 + 8 + 1 + 1;
}

#[account]
pub struct Batch {
    pub batch_no: u64,
    pub first_slot: u64,
    pub last_slot: u64,
    pub state_root: [u8; 32],
    /// Root of the L2's shallow withdrawals tree; `claim_withdrawal` verifies
    /// inclusion proofs against this (not the depth-256 `state_root`).
    pub withdrawals_root: [u8; 32],
    pub da_hash: [u8; 32],
    pub chunk_count: u32,
    pub posted_at: i64,
}
impl Batch {
    pub const SIZE: usize = 8 + 8 + 8 + 32 + 32 + 32 + 4 + 8;
}

#[account]
pub struct WithdrawalClaim {
    pub recipient: Pubkey,
    pub amount: u64,
    pub nonce: u64,
    pub claimed_at: i64,
}
impl WithdrawalClaim {
    pub const SIZE: usize = 32 + 8 + 8 + 8;
}

// ----- events -------------------------------------------------------------

#[event]
pub struct BatchPosted {
    pub batch_no: u64,
    pub first_slot: u64,
    pub last_slot: u64,
    pub state_root: [u8; 32],
    pub da_hash: [u8; 32],
}

#[event]
pub struct Deposited {
    pub depositor: Pubkey,
    pub amount: u64,
    pub l2_recipient: Pubkey,
    /// None = native SOL; Some = the SPL mint deposited.
    pub mint: Option<Pubkey>,
    /// Token decimals (9 for native SOL/ZUL), so the L2 wrapped mint matches.
    pub decimals: u8,
}

#[event]
pub struct Withdrawn {
    pub recipient: Pubkey,
    pub amount: u64,
    pub nonce: u64,
}

// ----- errors -------------------------------------------------------------

#[error_code]
pub enum SettlementError {
    #[msg("only the sequencer authority may call this")]
    Unauthorized,
    #[msg("batches must be posted sequentially")]
    NonSequentialBatch,
    #[msg("invalid slot range")]
    InvalidSlotRange,
    #[msg("amount must be positive")]
    ZeroAmount,
    #[msg("arithmetic overflow")]
    Overflow,
    #[msg("withdrawal proof did not verify against the posted state root")]
    InvalidProof,
}
