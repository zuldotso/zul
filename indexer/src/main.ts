// Indexer entrypoint: poll the ZUL node and persist into Postgres.
//
// Env:
//   ZUL_RPC_URL   node JSON-RPC endpoint (default http://127.0.0.1:8899)
//   POLL_MS       poll interval (default 1000)
//   DATABASE_URL  Postgres (Prisma)

import { prisma } from "@zul/db";
import { RpcClient } from "./rpc.js";
import { Indexer } from "./indexer.js";

const RPC_URL = process.env.ZUL_RPC_URL ?? "http://127.0.0.1:8899";
const POLL_MS = Number(process.env.POLL_MS ?? 1000);

async function main(): Promise<void> {
  const rpc = new RpcClient(RPC_URL);
  const indexer = new Indexer({ prisma, rpc });
  console.log(`ZUL indexer: ${RPC_URL} -> Postgres, polling every ${POLL_MS}ms`);

  let running = true;
  const shutdown = () => {
    running = false;
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);

  while (running) {
    try {
      const n = await indexer.tick();
      if (n > 0) console.log(`indexed ${n} slot(s)`);
    } catch (err) {
      console.error("index tick failed:", err instanceof Error ? err.message : err);
    }
    await sleep(POLL_MS);
  }

  await prisma.$disconnect();
  console.log("indexer stopped");
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
