import {
  getNetworkSnapshot,
  getRecentBlocks,
  getRecentTransactions,
} from "@/lib/queries";
import { formatCount, shorten } from "@/lib/format";
import { BlockList, TxList } from "./components/rows";
import { LiveSlot } from "./components/LiveSlot";

// Always render fresh; the chain moves.
export const dynamic = "force-dynamic";

export default async function HomePage() {
  const [net, blocks, txs] = await Promise.all([
    getNetworkSnapshot().catch(() => null),
    getRecentBlocks().catch(() => []),
    getRecentTransactions().catch(() => []),
  ]);

  return (
    <>
      <section style={{ marginBottom: 30 }}>
        <p className="page-head eyebrow" style={{ margin: 0 }}>
          Privacy Layer 2 · real SVM · native ZUL gas
        </p>
        <h1 style={{ fontSize: 34, margin: "6px 0 0", maxWidth: "20ch" }}>
          A precision view of the shielded chain.
        </h1>
      </section>

      <div className="stat-grid">
        <div className="stat">
          <div className="label">Slot height</div>
          <LiveSlot initial={net?.slot ?? 0} />
          <div className="meta">one block per slot</div>
        </div>
        <div className="stat">
          <div className="label">Transactions</div>
          <div className="value mono">{formatCount(net?.totalTransactions ?? 0)}</div>
          <div className="meta">executed on the L2</div>
        </div>
        <div className="stat mint">
          <div className="label">Shielded notes</div>
          <div className="value mono">{formatCount(net?.shieldedCommitments ?? 0)}</div>
          <div className="meta">{formatCount(net?.shieldedNullifiers ?? 0)} nullifiers spent</div>
        </div>
        <div className="stat mint">
          <div className="label">Pool root</div>
          <div className="value mono" style={{ fontSize: 18 }}>
            {net?.poolRoot ? shorten(net.poolRoot, 6, 6) : "—"}
          </div>
          <div className="meta">commitment tree state</div>
        </div>
      </div>

      <div className="two-col">
        <div className="card">
          <div className="section-title card-pad" style={{ paddingBottom: 0 }}>
            <h2>Latest blocks</h2>
            <a href="/blocks">View all →</a>
          </div>
          <BlockList blocks={blocks} />
        </div>
        <div className="card">
          <div className="section-title card-pad" style={{ paddingBottom: 0 }}>
            <h2>Latest transactions</h2>
            <a href="/txs">View all →</a>
          </div>
          <TxList txs={txs} />
        </div>
      </div>
    </>
  );
}
