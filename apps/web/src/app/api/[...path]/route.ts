import { NextRequest } from "next/server";

export const dynamic = "force-dynamic";

async function proxy(request: NextRequest, context: { params: Promise<{ path: string[] }> }) {
  const { path } = await context.params;
  const endpoint = path.join("/");
  const allowed = request.method === "GET"
    ? ["healthz", "readyz", "auth/me", "documents", "overview", "tools", "memories", "conversations"]
    : request.method === "POST" ? ["auth/login", "auth/logout", "documents", "knowledge/search", "knowledge/answer", "tools/knowledge_search", "tools/file_reader", "memories", "conversations"] : [];
  const conversationDetail = ["GET", "DELETE"].includes(request.method) && /^conversations\/[a-f0-9-]{36}$/i.test(endpoint);
  const conversationMessages = ["GET", "POST"].includes(request.method) && /^conversations\/[a-f0-9-]{36}\/messages$/i.test(endpoint);
  const conversationReplies = (request.method === "POST" && /^conversations\/[a-f0-9-]{36}\/replies(?:\/[a-f0-9-]{36}\/cancel)?$/i.test(endpoint))
    || (request.method === "GET" && /^conversations\/[a-f0-9-]{36}\/replies(?:\/[a-f0-9-]{36})?$/i.test(endpoint));
  const agentPlans = (request.method === "GET" && /^conversations\/[a-f0-9-]{36}\/agent-plans(?:\/[a-f0-9-]{36})?$/i.test(endpoint))
    || (request.method === "POST" && /^conversations\/[a-f0-9-]{36}\/agent-plans(?:\/[a-f0-9-]{36}\/(?:approve|cancel))?$/i.test(endpoint));
  const modelAgents = (request.method === "GET" && /^conversations\/[a-f0-9-]{36}\/model-agents(?:\/[a-f0-9-]{36})?$/i.test(endpoint))
    || (request.method === "POST" && /^conversations\/[a-f0-9-]{36}\/model-agents(?:\/[a-f0-9-]{36}\/(?:approve-planning|approve-execution|preview-execution|cancel-planning|cancel-execution))?$/i.test(endpoint));
  const schedules = (request.method === "GET" && /^(?:schedules(?:\/[a-f0-9-]{36})?|reminders)$/i.test(endpoint))
    || (request.method === "PUT" && /^reminders\/[a-f0-9-]{36}$/i.test(endpoint))
    || (request.method === "POST" && /^schedules(?:\/[a-f0-9-]{36}\/(?:approve|cancel))?$/i.test(endpoint));
  const feeds = (request.method === "GET" && /^(?:feeds\/config|feed-subscriptions(?:\/[a-f0-9-]{36}(?:\/entries)?)?|(?:feed-collections|feed-schedules)(?:\/[a-f0-9-]{36}(?:\/audit)?)?)$/i.test(endpoint))
    || (request.method === "POST" && /^(?:feed-subscriptions(?:\/[a-f0-9-]{36}\/(?:collections|schedules))?|feed-schedules\/[a-f0-9-]{36}\/(?:approve|cancel)|feed-collections\/[a-f0-9-]{36}\/(?:confirm|cancel|recover))$/i.test(endpoint))
    || (["PUT", "DELETE"].includes(request.method) && /^feed-subscriptions\/[a-f0-9-]{36}$/i.test(endpoint));
  const briefs = (["GET", "PUT"].includes(request.method) && ["feed-brief-preferences", "feed-brief-schedule"].includes(endpoint))
    || (["GET", "POST"].includes(request.method) && endpoint === "feed-briefs")
    || (["GET", "DELETE"].includes(request.method) && /^feed-briefs\/[a-f0-9-]{36}$/i.test(endpoint));
  const learning = (request.method === "GET" && /^learning\/(?:snapshot|progress|plans(?:\/[a-f0-9-]{36})?)$/i.test(endpoint))
    || (["PUT", "DELETE"].includes(request.method) && /^learning\/skills\/[a-f0-9-]{36}$/i.test(endpoint))
    || (request.method === "DELETE" && /^learning\/plans\/[a-f0-9-]{36}$/i.test(endpoint))
    || (request.method === "POST" && /^learning\/(?:assessments|plans(?:\/[a-f0-9-]{36}\/tasks\/[a-f0-9-]{36}\/result)?)$/i.test(endpoint));
  const learningEvidence = ["POST", "DELETE"].includes(request.method) && /^learning\/plans\/[a-f0-9-]{36}\/tasks\/[a-f0-9-]{36}\/evidence$/i.test(endpoint);
  const learningAuthorization = (request.method === "GET" && /^learning\/model-authorizations(?:\/[a-f0-9-]{36})?$/i.test(endpoint))
    || (request.method === "POST" && /^learning\/(?:model-authorizations\/[a-f0-9-]{36}\/(?:approve|cancel)|plans\/[a-f0-9-]{36}\/tasks\/[a-f0-9-]{36}\/evidence\/model-authorizations)$/i.test(endpoint));
  const learningPreview = request.method === "GET" && /^learning\/plans\/[a-f0-9-]{36}\/tasks\/[a-f0-9-]{36}\/evidence\/model-preview$/i.test(endpoint);
  const learningReview = request.method === "POST" && /^learning\/plans\/[a-f0-9-]{36}\/tasks\/[a-f0-9-]{36}\/evidence\/review(?:\/confirm)?$/i.test(endpoint);
  const subscriptionConnections = (request.method === "GET" && /^subscription-connections(?:\/[a-f0-9-]{36})?$/i.test(endpoint))
    || (request.method === "POST" && /^subscription-connections\/[a-f0-9-]{36}\/revoke$/i.test(endpoint));
  const feedValues = (request.method === "GET" && /^feed-values(?:\/[a-f0-9-]{36}(?:\/(?:audit|reading))?)?$/i.test(endpoint))
    || (request.method === "POST" && /^feed-values(?:\/[a-f0-9-]{36}\/(?:approve|cancel))?$/i.test(endpoint));
  const feedConfirmation = request.method === "POST" && /^feed-collections\/[a-f0-9-]{36}\/confirm$/i.test(endpoint);
  const mcpCredentials = (["GET", "POST"].includes(request.method) && endpoint === "mcp/credentials")
    || (request.method === "POST" && /^mcp\/credentials\/[a-f0-9-]{36}\/revoke$/i.test(endpoint));
  const memoryMutation = ["PUT", "DELETE"].includes(request.method) && /^memories\/[a-f0-9-]{36}$/i.test(endpoint);
  const documentDetail = request.method === "GET" && /^documents\/[a-f0-9-]{36}$/i.test(endpoint);
  const documentIndex = request.method === "POST" && /^documents\/[a-f0-9-]{36}\/index$/i.test(endpoint);
  const documentIndexJob = ["GET", "POST"].includes(request.method) && /^documents\/[a-f0-9-]{36}\/index-job$/i.test(endpoint);
  const toolAudit = request.method === "GET" && (endpoint === "tool-calls" || /^tool-calls\/[a-f0-9-]{36}$/i.test(endpoint));
  if (!allowed.includes(endpoint) && !documentDetail && !documentIndex && !documentIndexJob && !memoryMutation && !conversationDetail && !conversationMessages && !conversationReplies && !agentPlans && !modelAgents && !toolAudit && !schedules && !mcpCredentials && !feeds && !briefs && !learning && !learningEvidence && !learningReview && !learningPreview && !learningAuthorization && !subscriptionConnections && !feedValues) {
    return Response.json({ error: { code: "not_found" } }, { status: 404 });
  }
  // 必须携带非简单请求头；不得替不可信请求自动补充该请求头。
  if (request.method !== "GET" && request.headers.get("x-requested-with") !== "personal-ai") {
    return Response.json({ error: { code: "csrf_rejected" } }, { status: 403 });
  }
  const headers = new Headers();
  for (const name of ["content-type", "cookie", "x-requested-with"]) {
    const value = request.headers.get(name);
    if (value) headers.set(name, value);
  }
  if (request.method === "POST" && ["tools/knowledge_search", "tools/file_reader"].includes(endpoint)) {
    const requestId = request.headers.get("idempotency-key");
    if (requestId) headers.set("idempotency-key", requestId);
  }
  try {
    const limit = endpoint === "documents" ? 8 * 1024 * 1024 : learningEvidence && request.method === "POST" ? 64 * 1024 : 16384;
    let body: string | undefined;
    if (request.method !== "GET" && request.body) {
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
      { method: request.method, headers, body, cache: "no-store", redirect: "error", signal: AbortSignal.timeout(feedConfirmation ? 25000 : endpoint === "knowledge/answer" ? 60000 : (documentIndex || endpoint === "knowledge/search" || ["tools/knowledge_search", "tools/file_reader"].includes(endpoint)) ? 40000 : 10000) },
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
export const PUT = proxy;
export const DELETE = proxy;
