import { Connection, PublicKey, type ConnectionConfig } from "@solana/web3.js";
export declare const POOL_PROGRAM_ID: PublicKey;
export declare const LAMPORTS_PER_PL2 = 1000000000n;
export declare function connect(endpoint: string, config?: ConnectionConfig): Connection;
