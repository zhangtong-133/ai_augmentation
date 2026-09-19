// 仅用于隔离验收的确定性 HTTP 服务，不连接任何外部模型。
import { createServer } from "node:http";
createServer(async (request, response) => {
  if (request.method !== "POST" || !["/v1/embeddings", "/v1/chat/completions"].includes(request.url)) {
    response.writeHead(404).end(); return;
  }
  if (request.headers.authorization !== "Bearer fixture-only-key") {
    response.writeHead(401).end(); return;
  }
  const buffers = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > 128 * 1024) { response.writeHead(413).end(); return; }
    buffers.push(chunk);
  }
  try {
    const input = JSON.parse(Buffer.concat(buffers));
    if (request.url === "/v1/chat/completions") {
      if (input.model !== "fixture-chat" || input.store !== false || input.stream !== false ||
          input.max_completion_tokens !== 2048 || !input.response_format?.json_schema?.strict || input.tools) {
        response.writeHead(400).end(); return;
      }
      const context = JSON.parse(input.messages[1].content);
      if (!context.evidence?.length || context.evidence.length > 5) { response.writeHead(400).end(); return; }
      const answer = { answer: "依据资料：" + context.evidence[0].text.slice(0, 80), citations: [context.evidence[0].id], insufficient_evidence: false };
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ choices: [{ finish_reason: "stop", message: { content: JSON.stringify(answer), refusal: null } }] }));
      return;
    }
    if (input.model !== "fixture-embedding" || input.dimensions !== 3 || input.encoding_format !== "float" || !Array.isArray(input.input)) {
      response.writeHead(400).end(); return;
    }
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ model: input.model, data: input.input.map((text, index) => ({ index, embedding: [1, text.length % 7, text.length % 11] })).reverse() }));
  } catch { response.writeHead(400).end(); }
}).listen(8081, "0.0.0.0");
