"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

/// Routes a query to the right detail page by shape: all-digits -> block,
/// 64-ish base58 -> tx signature, otherwise -> account address.
export function SearchBar() {
  const router = useRouter();
  const [value, setValue] = useState("");

  function submit(e: React.FormEvent) {
    e.preventDefault();
    const q = value.trim();
    if (!q) return;
    if (/^\d+$/.test(q)) {
      router.push(`/block/${q}`);
    } else if (q.length >= 80) {
      router.push(`/tx/${q}`);
    } else {
      router.push(`/account/${q}`);
    }
    setValue("");
  }

  return (
    <form className="search" onSubmit={submit}>
      <span className="icon" aria-hidden>
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
          <circle cx="11" cy="11" r="7" />
          <path d="m21 21-4.3-4.3" strokeLinecap="round" />
        </svg>
      </span>
      <input
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="Slot, signature, or address"
        spellCheck={false}
        aria-label="Search the chain"
      />
    </form>
  );
}
