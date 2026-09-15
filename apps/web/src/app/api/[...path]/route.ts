import { NextRequest } from "next/server";

export const dynamic = "force-dynamic";

async function proxy(request: NextRequest, context: { params: Promise<{ path: string[] }> }) {
  const { path } = await context.params;
  const endpoint = path.join("/");
  const allowed = request.method === "GET"
    ? ["healthz", "readyz", "auth/me", "documents", "overview"]
    : ["auth/login", "auth/logout", "documents"];
  const documentDetail = request.method === "GET" && /^documents\/[a-f0-9-]{36}$/i.test(endpoint);
  if (!allowed.includes(endpoint) && !documentDetail) {
    return Response.json({ error: { code: "not_found" } }, { status: 404 });
  }
  // Require a non-simple header; never add it on behalf of an untrusted request.
  if (request.method === "POST" && request.headers.get("x-requested-with") !== "personal-ai") {
    return Response.json({ error: { code: "csrf_rejected" } }, { status: 403 });
  }
  const headers = new Headers();
  for (const name of ["content-type", "cookie", "x-requested-with"]) {
    const value = request.headers.get(name);
    if (value) headers.set(name, value);
  }
  try {
    const limit = endpoint === "documents" ? 8 * 1024 * 1024 : 16384;
    let body: string | undefined;
    if (request.method === "POST" && request.body) {
      const reader = request.body.getReader();
      const decoder = new TextDecoder("utf-8", { fatal: true });
      let size = 0;
      body = "";
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > limit) {
          await reader.cancel();
          return Response.json({ error: { code: "payload_too_large" } }, { status: 413 });
        }
        body += decoder.decode(value, { stream: true });
      }
      body += decoder.decode();
    }
    const response = await fetch(
      new URL("/api/" + endpoint + request.nextUrl.search, process.env.API_INTERNAL_URL ?? "http://127.0.0.1:8080"),
      { method: request.method, headers, body, cache: "no-store", redirect: "error", signal: AbortSignal.timeout(10000) },
    );
    const output = new Headers({ "Content-Type": "application/json", "Cache-Control": "no-store" });
    const cookie = response.headers.get("set-cookie");
    if (cookie) output.set("set-cookie", cookie);
    return new Response(response.body, { status: response.status, headers: output });
  } catch {
    return Response.json({ error: { code: "api_unavailable" } }, { status: 503, headers: { "Cache-Control": "no-store" } });
  }
}

export const GET = proxy;
export const POST = proxy;
