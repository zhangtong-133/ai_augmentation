"use client";

import { useEffect, useState, type FormEvent } from "react";
import { KnowledgePanel } from "./knowledge-panel";

type User = { id: string; email: string; display_name: string };

export function AccountPanel() {
  const [user, setUser] = useState<User | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("正在检查服务…");
  const [error, setError] = useState("");

  useEffect(() => {
    const controller = new AbortController();
    async function load() {
      try {
        const [ready, identity] = await Promise.all([
          fetch("/api/readyz", { cache: "no-store", signal: controller.signal }),
          fetch("/api/auth/me", { cache: "no-store", signal: controller.signal }),
        ]);
        setStatus(ready.ok ? "服务已连接" : "服务暂不可用");
        if (identity.ok) setUser(await identity.json());
        else if (identity.status !== 401) setError("暂时无法读取账户，请稍后刷新。");
      } catch {
        if (!controller.signal.aborted) setStatus("无法连接服务");
      } finally {
        if (!controller.signal.aborted) setLoading(false);
      }
    }
    void load();
    return () => controller.abort();
  }, []);

  async function login(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    setBusy(true);
    setError("");
    try {
      const response = await fetch("/api/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        body: JSON.stringify({ email: data.get("email"), password: data.get("password") }),
      });
      if (!response.ok) {
        setError(response.status === 401 ? "邮箱或密码不正确。" :
          response.status === 429 ? "尝试过于频繁，请一分钟后再试。" : "暂时无法登录，请检查服务连接。");
        return;
      }
      form.reset();
      const identity = await fetch("/api/auth/me", { cache: "no-store" });
      if (!identity.ok) { setError("无法保持登录，请检查浏览器 Cookie 设置与服务配置。"); return; }
      setUser(await identity.json());
      setStatus("服务已连接");
    } catch { setError("网络连接失败，请稍后重试。"); }
    finally { setBusy(false); }
  }

  async function logout() {
    setBusy(true);
    setError("");
    try {
      const response = await fetch("/api/auth/logout", {
        method: "POST", headers: { "X-Requested-With": "personal-ai" },
      });
      if (!response.ok) throw new Error("logout failed");
      setUser(null);
    } catch { setError("退出未完成，请重试。"); }
    finally { setBusy(false); }
  }

  return (
    <section className="accountPanel" aria-label="个人账户">
      <p role="status">{status}</p>
      {loading ? <p>正在读取账户…</p> : user ? (
        <div>
          <h2>欢迎回来，{user.display_name}</h2>
          <p>{user.email}</p>
          <button disabled={busy} onClick={() => void logout()}>{busy ? "处理中…" : "退出登录"}</button>
        </div>
      ) : (
        <form onSubmit={(event) => void login(event)}>
          <h2>登录你的工作台</h2>
          <label htmlFor="email">邮箱</label>
          <input id="email" name="email" type="email" autoComplete="username" maxLength={254} required disabled={busy} />
          <label htmlFor="password">密码</label>
          <input id="password" name="password" type="password" autoComplete="current-password" maxLength={256} required disabled={busy} />
          <button disabled={busy}>{busy ? "登录中…" : "登录"}</button>
          <small>首次使用请先按项目文档创建账户并设置密码。</small>
        </form>
      )}
      {error && <p role="alert">{error}</p>}
      {user && <KnowledgePanel key={user.id} />}
    </section>
  );
}
