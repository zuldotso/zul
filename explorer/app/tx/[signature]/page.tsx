import { notFound } from "next/navigation";
import { getTransaction } from "@/lib/queries";
import { formatZul } from "@/lib/format";
import { StatusBadge } from "@/app/components/rows";

export const dynamic = "force-dynamic";

export default async function TxPage({
  params,
}: {
  params: Promise<{ signature: string }>;
}) {
  const { signature } = await params;
  const tx = await getTransaction(decodeURIComponent(signature)).catch(() => null);
  if (!tx) notFound();

  return (
    <>
      <div className="page-head">
        <div style={{ minWidth: 0 }}>
          <div className="eyebrow">Transaction</div>
          <h1 className="mono" style={{ fontSize: 20, wordBreak: "break-all" }}>
            {tx.signature}
          </h1>
        </div>
        <div style={{ marginLeft: "auto", display: "flex", gap: 10, alignItems: "center" }}>
          <StatusBadge success={tx.success} />
          {tx.touchedPool && <span className="badge pool">shielded pool</span>}
        </div>
      </div>

      <div className="kv" style={{ marginBottom: 24 }}>
        <div className="k">Block</div>
        <div className="v">
          <a className="hash" href={`/block/${tx.slot}`}>#{tx.slot}</a>{" "}
          <span className="cell-dim">· index {tx.indexInBlock}</span>
        </div>
        <div className="k">Fee</div>
        <div className="v">{formatZul(tx.fee)} ZUL</div>
        <div className="k">Compute units</div>
        <div className="v mono">{Number(tx.computeUnits).toLocaleString()}</div>
        <div className="k">Fee payer</div>
        <div className="v mono">
          <a className="hash" href={`/account/${tx.feePayer}`}>{tx.feePayer}</a>
        </div>
        <div className="k">Programs</div>
        <div className="v">
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            {tx.programs.length === 0 && <span className="cell-dim">—</span>}
            {tx.programs.map((p, i) => (
              <span key={i} className="badge neutral">{p}</span>
            ))}
          </div>
        </div>
        {tx.err && (
          <>
            <div className="k">Error</div>
            <div className="v mono" style={{ color: "var(--red)" }}>{tx.err}</div>
          </>
        )}
      </div>

      <div className="grid" style={{ gridTemplateColumns: "1fr", gap: 24 }}>
        <div>
          <div className="section-title"><h2>Accounts</h2></div>
          <div className="kv">
            {tx.accountKeys.map((k, i) => (
              <>
                <div className="k" key={`k${i}`}>#{i}</div>
                <div className="v mono" key={`v${i}`}>
                  <a className="hash" href={`/account/${k}`}>{k}</a>
                </div>
              </>
            ))}
            {tx.accountKeys.length === 0 && (
              <>
                <div className="k">—</div>
                <div className="v cell-dim">no account keys indexed</div>
              </>
            )}
          </div>
        </div>

        <div>
          <div className="section-title"><h2>Program logs</h2></div>
          {tx.logMessages.length > 0 ? (
            <div className="log">
              {tx.logMessages.map((line, i) => (
                <div key={i}>
                  {line.startsWith("Program ") ? <span className="prog">{line}</span> : line}
                </div>
              ))}
            </div>
          ) : (
            <div className="empty">No logs recorded.</div>
          )}
        </div>
      </div>
    </>
  );
}
