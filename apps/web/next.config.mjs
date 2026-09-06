/** @type {import('next').NextConfig} */
const nextConfig = {
  output: "standalone",
  reactStrictMode: true,
  experimental: {
    // Keep type checking in-process; some restricted WSL sandboxes cannot
    // reliably capture the detached TypeScript CLI child process.
    useTypeScriptCli: false,
  },
};

export default nextConfig;
