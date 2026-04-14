import type { Metadata } from "next";
import { Space_Grotesk, Inter, JetBrains_Mono } from "next/font/google";
import { SearchBar } from "./components/SearchBar";
import "./globals.css";

const display = Space_Grotesk({
  subsets: ["latin"],
  variable: "--font-display",
  weight: ["400", "500", "600"],
});
const body = Inter({ subsets: ["latin"], variable: "--font-body" });
const mono = JetBrains_Mono({ subsets: ["latin"], variable: "--font-mono" });

export const metadata: Metadata = {
  title: "Zul Explorer — privacy Layer 2 on Solana",
  description:
    "Block explorer for Zul: a real SVM Layer 2 with native gas and a multi-asset shielded pool.",
};

const NAV = [
  { href: "/", label: "Overview" },
  { href: "/blocks", label: "Blocks" },
  { href: "/txs", label: "Transactions" },
  { href: "/shielded", label: "Shielded pool" },
];

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body className={`${display.variable} ${body.variable} ${mono.variable}`}>
        <header className="topbar">
          <div className="shell topbar-inner">
            <a href="/" className="brand">
              <span className="mark" aria-hidden />
              Zul
              <span className="sub">explorer</span>
            </a>
            <nav className="nav">
              {NAV.map((n) => (
                <a key={n.href} href={n.href}>
                  {n.label}
                </a>
              ))}
            </nav>
            <SearchBar />
          </div>
        </header>
        <main>
          <div className="shell">{children}</div>
        </main>
        <footer className="footer">
          <div className="shell" style={{ display: "flex", justifyContent: "space-between", width: "100%", gap: 16, flexWrap: "wrap" }}>
            <span>Zul — privacy Layer 2 on Solana · Stage-0 rollup, single sequencer</span>
            <span className="mono">native gas: ZUL · shielded: ZUL + wSOL</span>
          </div>
        </footer>
      </body>
    </html>
  );
}
