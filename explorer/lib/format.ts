// Display formatting helpers. The explorer deals in BigInt (Prisma) which
// must never reach a client component raw — convert at the server boundary.

export const LAMPORTS_PER_ZUL = 1_000_000_000n;

export function shorten(value: string, head = 6, tail = 6): string {
  if (value.length <= head + tail + 1) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}

export function formatZul(lamports: bigint | string): string {
  const v = typeof lamports === "string" ? BigInt(lamports) : lamports;
  const whole = v / LAMPORTS_PER_ZUL;
  const frac = v % LAMPORTS_PER_ZUL;
  if (frac === 0n) return `${whole.toLocaleString()}`;
  const fracStr = frac.toString().padStart(9, "0").replace(/0+$/, "");
  return `${whole.toLocaleString()}.${fracStr}`;
}

export function timeAgo(timestampMs: number): string {
  const delta = Date.now() - timestampMs;
  const s = Math.floor(delta / 1000);
  if (s < 5) return "just now";
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
}

export function formatCount(n: bigint | number): string {
  return Number(n).toLocaleString();
}
