const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
JSON.parse(Buffer.concat(chunks).toString("utf8"));
const mode = process.argv[2] ?? "allow";
if (mode === "invalid") process.stdout.write("not-json");
else process.stdout.write(JSON.stringify({ decision: mode === "block" ? "block" : "allow", message: mode === "block" ? "blocked by fixture" : "", output: mode === "replace" ? "hook output" : null }));
