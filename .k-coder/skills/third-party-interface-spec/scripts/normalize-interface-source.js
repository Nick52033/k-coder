#!/usr/bin/env node

const fs = require("fs");

function usage() {
  console.error("Usage: node normalize-interface-source.js --type idata|curl --input <file> [--base-url <url>]");
  process.exit(2);
}

function readArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 1) {
    const key = argv[i];
    const value = argv[i + 1];
    if (!key.startsWith("--") || value === undefined || value.startsWith("--")) usage();
    args[key.slice(2)] = value;
    i += 1;
  }
  if (!args.type || !args.input) usage();
  return args;
}

function tryJson(value) {
  if (typeof value !== "string") return value;
  const trimmed = value.trim();
  if (!trimmed || !/^[\[{]/.test(trimmed)) return value;
  try {
    return JSON.parse(trimmed);
  } catch {
    return value;
  }
}

function findApiObject(value, depth = 0) {
  if (depth > 8 || value == null) return null;
  const parsed = tryJson(value);
  if (Array.isArray(parsed)) {
    for (const item of parsed) {
      const found = findApiObject(item, depth + 1);
      if (found) return found;
    }
    return null;
  }
  if (typeof parsed !== "object") return null;
  if (parsed.apiPath || parsed.apiParams || parsed.apiResults) return parsed;
  for (const key of ["data", "result", "payload", "body", "rows", "records", "list"]) {
    const found = findApiObject(parsed[key], depth + 1);
    if (found) return found;
  }
  for (const value of Object.values(parsed)) {
    const found = findApiObject(value, depth + 1);
    if (found) return found;
  }
  return null;
}

function firstDefined(obj, keys) {
  for (const key of keys) {
    if (obj && obj[key] !== undefined && obj[key] !== null && obj[key] !== "") return obj[key];
  }
  return "";
}

function normalizeRequired(value, obj) {
  if (value === undefined || value === null || value === "") {
    if (obj && obj.nullable !== undefined) return obj.nullable === false || obj.nullable === "false" ? "是" : "否";
    return "";
  }
  if (typeof value === "boolean") return value ? "是" : "否";
  const text = String(value).trim().toLowerCase();
  if (["1", "y", "yes", "true", "required", "must", "是", "必填", "√"].includes(text)) return "是";
  if (["0", "n", "no", "false", "optional", "否", "非必填"].includes(text)) return "否";
  return String(value);
}

function inferTypeFromExample(value) {
  if (value === undefined || value === null || value === "") return "";
  if (Array.isArray(value)) return "array(推断)";
  if (typeof value === "boolean") return "boolean(推断)";
  if (typeof value === "number") return Number.isInteger(value) ? "integer(推断)" : "decimal(推断)";
  if (typeof value === "object") return "object(推断)";
  const text = String(value);
  if (/^\d{4}-\d{2}-\d{2}/.test(text) || /^\d{8}$/.test(text)) return "date/string(推断)";
  if (/^-?\d+(\.\d+)?$/.test(text)) return "number/string(推断)";
  return "string(推断)";
}

function normalizeFields(raw, basePath = "") {
  const parsed = tryJson(raw);
  const list = Array.isArray(parsed) ? parsed : parsed && typeof parsed === "object" ? Object.values(parsed) : [];
  const rows = [];

  for (const item of list) {
    if (!item || typeof item !== "object") continue;
    const name = firstDefined(item, [
      "paramName",
      "name",
      "fieldName",
      "field",
      "columnName",
      "code",
      "paramCode",
      "property",
      "key"
    ]);
    const path = name ? (basePath ? `${basePath}.${name}` : String(name)) : basePath;
    const example = firstDefined(item, ["example", "exampleValue", "sample", "value", "defaultValue"]);
    const rawType = firstDefined(item, ["paramType", "dataType", "type", "fieldType", "columnType", "javaType"]);
    const children = firstDefined(item, ["children", "childList", "properties", "columns", "items", "fields", "params"]);
    const row = {
      path,
      name: String(name || ""),
      title: String(firstDefined(item, ["paramDesc", "description", "desc", "fieldDesc", "title", "label", "comment", "remark", "memo"]) || ""),
      rawType: String(rawType || inferTypeFromExample(example)),
      required: normalizeRequired(firstDefined(item, ["required", "isRequired", "must", "requiredFlag", "notNull"]), item),
      defaultValue: String(firstDefined(item, ["defaultValue", "default", "fixedValue"]) || ""),
      enumOrRange: String(firstDefined(item, ["enum", "enumValue", "dict", "dictCode", "range", "valueRange"]) || ""),
      example: example === undefined || typeof example === "object" ? "" : String(example),
      note: String(firstDefined(item, ["note", "remark", "memo", "tips"]) || "")
    };
    rows.push(row);
    if (children) rows.push(...normalizeFields(children, path));
  }

  return rows;
}

function joinUrl(baseUrl, apiPath) {
  const base = (baseUrl || "https://ipsapro.isoftstone.com/iDataPlatform/idss/apimarket/onlineApi/getData").replace(/\/+$/, "");
  const path = String(apiPath || "").replace(/^\/+/, "");
  return path ? `${base}/${path}` : base;
}

function normalizeMethod(rawMethod) {
  if (rawMethod === undefined || rawMethod === null || rawMethod === "") {
    return {
      method: "POST",
      note: "apiMethod 缺失，已按 POST 处理，需确认。"
    };
  }
  const text = String(rawMethod).trim().toUpperCase();
  if (["GET", "POST", "PUT", "PATCH", "DELETE"].includes(text)) {
    return { method: text, note: "" };
  }
  if (text === "1") {
    return {
      method: "POST",
      note: "apiMethod 为 1，已按中台常见约定映射为 POST；如平台含义不同需确认。"
    };
  }
  if (text === "0") {
    return {
      method: "GET",
      note: "apiMethod 为 0，已按 GET 推断；如平台含义不同需确认。"
    };
  }
  return {
    method: text,
    note: `apiMethod=${text} 不是标准 HTTP 方法，需确认。`
  };
}

function normalizeIData(text, baseUrl) {
  const root = JSON.parse(text);
  const api = findApiObject(root);
  if (!api) throw new Error("Cannot find object containing apiPath/apiParams/apiResults.");
  const methodInfo = normalizeMethod(api.apiMethod);
  const apiPath = String(api.apiPath || "");
  const responseFields = normalizeFields(api.apiResults || []).map((row) => ({
    ...row,
    path: row.path ? `data[].${row.path}` : "data[]"
  }));
  return {
    sourceType: "idata-platform",
    apiName: firstDefined(api, ["apiName", "name", "serviceName", "title"]),
    apiPath,
    method: methodInfo.method,
    finalUrl: joinUrl(baseUrl, apiPath),
    requestFields: normalizeFields(api.apiParams || []),
    responseEnvelope: {
      message: null,
      msg: "接口访问成功",
      data: []
    },
    responseDataPath: "data[]",
    responseFields,
    notes: methodInfo.note ? [methodInfo.note] : []
  };
}

function tokenizeCurl(text) {
  const tokens = [];
  let current = "";
  let quote = null;
  let escaped = false;
  const normalized = text.replace(/\\\r?\n/g, " ");
  for (const ch of normalized) {
    if (escaped) {
      current += ch;
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      escaped = true;
      continue;
    }
    if (quote) {
      if (ch === quote) quote = null;
      else current += ch;
      continue;
    }
    if (ch === "'" || ch === "\"") {
      quote = ch;
      continue;
    }
    if (/\s/.test(ch)) {
      if (current) {
        tokens.push(current);
        current = "";
      }
      continue;
    }
    current += ch;
  }
  if (current) tokens.push(current);
  return tokens;
}

function redactHeaderValue(header) {
  return header.replace(/(authorization|token|cookie|secret|password)([^:]*:\s*)(.*)$/i, "$1$2***");
}

function normalizeCurl(text) {
  const tokens = tokenizeCurl(text);
  const result = {
    sourceType: "curl",
    method: "",
    url: "",
    headers: [],
    body: "",
    contentType: "",
    authLocation: []
  };

  for (let i = 0; i < tokens.length; i += 1) {
    const token = tokens[i];
    if (token === "curl") continue;
    if (!token.startsWith("-") && !result.url) {
      result.url = token;
      continue;
    }
    if (["-X", "--request"].includes(token)) {
      result.method = String(tokens[++i] || "").toUpperCase();
      continue;
    }
    if (["-H", "--header"].includes(token)) {
      const header = tokens[++i] || "";
      result.headers.push(redactHeaderValue(header));
      if (/^content-type\s*:/i.test(header)) result.contentType = header.split(":").slice(1).join(":").trim();
      if (/authorization|token|cookie/i.test(header)) result.authLocation.push(header.split(":")[0]);
      continue;
    }
    if (["-d", "--data", "--data-raw", "--data-binary", "--data-urlencode"].includes(token)) {
      result.body = tokens[++i] || "";
      if (!result.method) result.method = "POST";
      continue;
    }
  }

  if (!result.method) result.method = "GET";
  return result;
}

function main() {
  const args = readArgs(process.argv);
  const text = fs.readFileSync(args.input, "utf8");
  let output;
  if (args.type === "idata") output = normalizeIData(text, args["base-url"]);
  else if (args.type === "curl") output = normalizeCurl(text);
  else usage();
  console.log(JSON.stringify(output, null, 2));
}

main();
