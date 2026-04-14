// Server-side data access. Every function converts BigInt to plain numbers
// or strings so the result is safe to hand to client components.

import { prisma } from "@zul/db";

export interface NetworkSnapshot {
  slot: number;
  genesisHash: string;
  totalTransactions: number;
  shieldedCommitments: number;
  shieldedNullifiers: number;
  poolRoot: string;
}

export async function getNetworkSnapshot(): Promise<NetworkSnapshot> {
  const [sync, txCount, pool] = await Promise.all([
    prisma.syncState.findUnique({ where: { id: 1 } }),
    prisma.transaction.count(),
    prisma.poolStat.findUnique({ where: { id: 1 } }),
  ]);
  return {
    slot: Number(sync?.lastSlot ?? 0n),
    genesisHash: sync?.genesisHash ?? "",
    totalTransactions: txCount,
    shieldedCommitments: Number(pool?.commitmentCount ?? 0n),
    shieldedNullifiers: Number(pool?.nullifierCount ?? 0n),
    poolRoot: pool?.currentRoot ?? "",
  };
}

export interface BlockRow {
  slot: number;
  blockhash: string;
  txCount: number;
  timestampMs: number;
}

export async function getRecentBlocks(limit = 12): Promise<BlockRow[]> {
  const blocks = await prisma.block.findMany({
    orderBy: { slot: "desc" },
    take: limit,
  });
  return blocks.map((b) => ({
    slot: Number(b.slot),
    blockhash: b.blockhash,
    txCount: b.txCount,
    timestampMs: Number(b.timestampMs),
  }));
}

export interface TxRow {
  signature: string;
  slot: number;
  success: boolean;
  fee: string;
  feePayer: string;
  programs: string[];
  touchedPool: boolean;
}

export async function getRecentTransactions(limit = 12): Promise<TxRow[]> {
  const txs = await prisma.transaction.findMany({
    orderBy: [{ slot: "desc" }, { indexInBlock: "desc" }],
    take: limit,
  });
  return txs.map(toTxRow);
}

function toTxRow(t: {
  signature: string;
  slot: bigint;
  success: boolean;
  fee: bigint;
  feePayer: string;
  programs: string[];
  touchedPool: boolean;
}): TxRow {
  return {
    signature: t.signature,
    slot: Number(t.slot),
    success: t.success,
    fee: t.fee.toString(),
    feePayer: t.feePayer,
    programs: t.programs,
    touchedPool: t.touchedPool,
  };
}

export async function getBlock(slot: number) {
  const block = await prisma.block.findUnique({
    where: { slot: BigInt(slot) },
    include: { transactions: { orderBy: { indexInBlock: "asc" } } },
  });
  if (!block) return null;
  return {
    slot: Number(block.slot),
    blockhash: block.blockhash,
    parentHash: block.parentHash,
    stateRoot: block.stateRoot,
    txCount: block.txCount,
    timestampMs: Number(block.timestampMs),
    transactions: block.transactions.map(toTxRow),
  };
}

export async function getTransaction(signature: string) {
  const t = await prisma.transaction.findUnique({ where: { signature } });
  if (!t) return null;
  return {
    ...toTxRow(t),
    err: t.err,
    computeUnits: t.computeUnits.toString(),
    accountKeys: t.accountKeys,
    logMessages: t.logMessages,
    indexInBlock: t.indexInBlock,
  };
}

export async function getAccountActivity(address: string, limit = 25) {
  const sigs = await prisma.addressSignature.findMany({
    where: { address },
    orderBy: { slot: "desc" },
    take: limit,
  });
  const signatures = sigs.map((s) => s.signature);
  const txs = await prisma.transaction.findMany({
    where: { signature: { in: signatures } },
    orderBy: [{ slot: "desc" }, { indexInBlock: "desc" }],
  });
  return txs.map(toTxRow);
}

export async function getShieldedEvents(limit = 30) {
  const events = await prisma.shieldedEvent.findMany({
    orderBy: { slot: "desc" },
    take: limit,
  });
  return events.map((e) => ({
    slot: Number(e.slot),
    signature: e.signature,
    kind: e.kind,
    commitmentsAdded: e.commitmentsAdded,
    nullifiersSpent: e.nullifiersSpent,
    feePaid: e.feePaid.toString(),
  }));
}
