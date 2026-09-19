// 仅用于隔离验收的确定性 HTTP 服务，不连接任何外部模型。
import { createServer } from "node:http";
let failNext = false;
createServer(async (request, response) => {
  // 仅隔离测试容器内部使用的故障注入入口，不发布宿主端口。
  if (request.method === "POST" && request.url === "/control/fail-once") {
    failNext = true;
    response.writeHead(204).end(); return;
  }
  if (request.method !== "POST" || request.url !== "/v1/embeddings") {
    response.writeHead(404).end(); return;
  }
  if (request.headers.authorization !== "Bearer fixture-only-key") {
    response.writeHead(401).end(); return;
  }
  if (failNext) {
    failNext = false;
    response.writeHead(429).end(); return;
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
    if (input.model !== "fixture-embedding" || input.dimensions !== 3 || input.encoding_format !== "float" || !Array.isArray(input.input)) {
      response.writeHead(400).end(); return;
    }
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ model: input.model, data: input.input.map((text, index) => ({ index, embedding: [1, text.length % 7, text.length % 11] })).reverse() }));
  } catch { response.writeHead(400).end(); }
}).listen(8081, "0.0.0.0");
