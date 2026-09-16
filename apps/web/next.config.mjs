/** @type {import('next').NextConfig} */
const nextConfig = {
  output: "standalone",
  reactStrictMode: true,
  experimental: {
    // 在当前进程中执行类型检查；部分受限 WSL 沙箱无法
    // 可靠地获取独立 TypeScript CLI 子进程的执行结果。
    useTypeScriptCli: false,
  },
};

export default nextConfig;
