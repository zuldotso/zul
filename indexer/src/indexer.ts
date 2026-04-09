// Core indexing loop: walk new slots, persist blocks + transactions
// idempotently, and advance the sync cursor. Pure-ish: takes the RPC client
// and a Prisma client so it can be tested with fakes.

import type { PrismaClient } from "@zul/db";
import { RpcClient } from "./rpc.js";
import { decodeTransaction } from "./decode.js";

export interface IndexerDeps {
  prisma: PrismaClient;
  rpc: RpcClient;
  /// Max slots to process per tick (bounds catch-up work).
  batchSize?: number;
}

export class Indexer {
  private prisma: PrismaClient;
  private rpc: RpcClient;
  private batchSize: number;

  constructor(deps: IndexerDeps) {
    this.prisma = deps.prisma;
    this.rpc = deps.rpc;
    this.batchSize = deps.batchSize ?? 256;
  }

  async ensureSyncState(): Promise<bigint> {
    const genesisHash = await this.rpc.getGenesisHash().catch(() => "");
    const state = await this.prisma.syncState.upsert({
      where: { id: 1 },
      create: { id: 1, lastSlot: 0n, genesisHash },
      update: genesisHash ? { genesisHash } : {},
    });
    return state.lastSlot;
  }

  /// Process one tick. Returns the number of slots indexed.
  async tick(): Promise<number> {
    const lastSlot = await this.ensureSyncState();
    const head = BigInt(await this.rpc.getSlot());
    if (head <= lastSlot) return 0;

    const upTo = bigintMin(head, lastSlot + BigInt(this.batchSize));
    let indexed = 0;
    for (let slot = lastSlot + 1n; slot <= upTo; slot++) {
      await this.indexSlot(slot);
      indexed++;
    }
    await this.prisma.syncState.update({
      where: { id: 1 },
      data: { lastSlot: upTo },
    });
    return indexed;
  }

  async indexSlot(slot: bigint): Promise<void> {
    const block = await this.rpc.getBlock(Number(slot));
    if (!block) return; // empty/no block at this slot

    await this.prisma.block.upsert({
      where: { slot },
      create: {
        slot,
        blockhash: block.blockhash,
        parentHash: block.previousBlockhash,
        stateRoot: block.stateRoot ?? "",
        txCount: block.signatures.length,
        timestampMs: BigInt(block.blockTime) * 1000n,
      },
      update: { txCount: block.signatures.length },
    });

    for (let i = 0; i < block.signatures.length; i++) {
      await this.indexTransaction(slot, i, block.signatures[i]);
    }
  }

  async indexTransaction(slot: bigint, index: number, signature: string): Promise<void> {
    const tx = await this.rpc.getTransaction(signature);
    if (!tx || !tx.meta) return;

    const decoded = tx.transaction?.[0]
      ? decodeTransaction(tx.transaction[0])
      : null;
    const accountKeys = decoded?.accountKeys ?? tx.meta.accountKeys ?? [];
    const feePayer = decoded?.feePayer ?? accountKeys[0] ?? "";
    const programs = decoded?.programs ?? [];
    const touchedPool = decoded?.touchedPool ?? false;
    const err = tx.meta.err ? JSON.stringify(tx.meta.err) : null;

    await this.prisma.transaction.upsert({
      where: { signature },
      create: {
        signature,
        slot,
        indexInBlock: index,
        success: err === null,
        err,
        fee: BigInt(tx.meta.fee ?? 0),
        computeUnits: BigInt(tx.meta.computeUnitsConsumed ?? 0),
        feePayer,
        accountKeys,
        logMessages: tx.meta.logMessages ?? [],
        programs,
        touchedPool,
      },
      update: { indexInBlock: index, success: err === null, err },
    });

    // Address -> signature index (dedup on the unique pair).
    for (const address of accountKeys) {
      await this.prisma.addressSignature
        .create({ data: { address, signature, slot } })
        .catch(() => {
          /* unique violation = already indexed */
        });
    }
  }
}

function bigintMin(a: bigint, b: bigint): bigint {
  return a < b ? a : b;
}
