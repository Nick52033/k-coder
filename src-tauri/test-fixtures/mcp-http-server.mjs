import http from "node:http";

const server = http.createServer(async (request, response) => {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  const message = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  if (request.headers.authorization !== "Bearer hidden-value") {
    response.writeHead(401).end();
    return;
  }
  if (message.method === "notifications/initialized") {
    response.writeHead(202, { "MCP-Session-Id": "fixture-session" }).end();
    return;
  }
  let result;
  if (message.method === "initialize") {
    result = { protocolVersion: "2025-11-25", capabilities: { tools: {} }, serverInfo: { name: "http-fixture", version: "1.0.0" } };
  } else if (message.method === "tools/list") {
    result = { tools: [{ name: "remote_read", description: "Read remote data", inputSchema: { type: "object", additionalProperties: false }, annotations: { readOnlyHint: true, openWorldHint: true } }] };
  } else if (message.method === "tools/call") {
    result = { content: [{ type: "text", text: "remote result" }] };
  } else {
    response.writeHead(404).end();
    return;
  }
  response.writeHead(200, { "content-type": "application/json", "MCP-Session-Id": "fixture-session", connection: "close" });
  response.end(JSON.stringify({ jsonrpc: "2.0", id: message.id, result }));
});

server.listen(0, "127.0.0.1", () => {
  process.stdout.write(`${server.address().port}\n`);
});
