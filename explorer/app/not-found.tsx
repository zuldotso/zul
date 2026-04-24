export default function NotFound() {
  return (
    <div style={{ padding: "80px 0", textAlign: "center" }}>
      <div className="eyebrow" style={{ color: "var(--text-faint)", letterSpacing: "0.16em", textTransform: "uppercase", fontSize: 11 }}>
        404
      </div>
      <h1 style={{ fontSize: 32, margin: "10px 0 14px" }}>Not found on this chain</h1>
      <p style={{ color: "var(--text-dim)", marginBottom: 24 }}>
        That slot, signature, or address has not been indexed.
      </p>
      <a className="badge pool" href="/" style={{ height: 32, padding: "0 16px" }}>
        ← Back to overview
      </a>
    </div>
  );
}
