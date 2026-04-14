/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  // @zul/db ships TS source; let Next transpile the workspace package.
  transpilePackages: ["@zul/db"],
  experimental: {
    serverActions: { bodySizeLimit: "2mb" },
  },
};

export default nextConfig;
