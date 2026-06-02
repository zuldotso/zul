import { getNetworkSnapshot, getShieldedEvents } from "@/lib/queries";
import { formatCount, formatZul, shorten } from "@/lib/format";

export const dynamic = "force-dynamic";

const KIND_LABEL: Record<string, string> = {
  shield: "Shield",
  transfer: "Private transfer",
  unshield: "Unshield",
};

export default async function ShieldedPage() {
  const [net, events] = await Promise.all([
    getNetworkSnapshot().catch(() => null),
    getShieldedEvents().catch(() => []),
  ]);

  return (
    <>
      <div className="pool-hero">
        <div className="eyebrow" style={{ color: "var(--mint)", letterSpacing: "0.16em", textTransform: "uppercase", fontSize: 11, marginBottom: 10 }}>
          Multi-asset shielded pool
        </div>
        <h1>Value that moves without a trace.</h1>
        <p>
          Notes are Poseidon commitments in an append-only Merkle tree; spends
          reveal only nullifiers. A Groth16 proof over BN254 — generated in the
          browser, verified natively in the node — enforces ownership and value
          conservation. Asset identity stays private inside transfers.
        </p>
      </div>

      <div className="stat-grid" style={{ gridTemplateColumns: "repeat(3, 1fr)" }}>
        <div className="stat mint">
          <div className="label">Commitments</div>
          <div className="value mono">{formatCount(net?.shieldedCommitments ?? 0)}</div>
          <div className="meta">notes ever created</div>
        </div>
        <div className="stat mint">
          <div className="label">Nullifiers</div>
          <div className="value mono">{formatCount(net?.shieldedNullifiers ?? 0)}</div>
          <div className="meta">notes spent</div>
        </div>
        <div className="stat">
          <div className="label">Current root</div>
          <div className="value mono" style={{ fontSize: 16 }}>
            {net?.poolRoot ? shorten(net.poolRoot, 8, 8) : "—"}
          </div>
          <div className="meta">depth-26 tree</div>
        </div>
      </div>

      <div className="card" style={{ marginTop: 8 }}>
        <div className="section-title card-pad" style={{ paddingBottom: 0 }}>
          <h2>Recent pool activity</h2>
          <span className="cell-dim">public counters only</span>
        </div>
        {events.length === 0 ? (
          <div className="empty">
            No shielded activity yet. Pool events appear once the privacy
            program is active and the indexer parses its logs.
          </div>
        ) : (
          <div className="rows">
            {events.map((e, i) => (
              <a key={i} href={`/tx/${e.signature}`} className="row" style={{ gridTemplateColumns: "150px 1fr 110px 110px" }}>
                <span className="badge pool">{KIND_LABEL[e.kind] ?? e.kind}</span>
                <span className="hash dim">{shorten(e.signature, 10, 8)}</span>
                <span className="cell-dim">+{e.commitmentsAdded} / −{e.nullifiersSpent}</span>
                <span className="cell-dim">{formatZul(e.feePaid)} ZUL</span>
              </a>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
