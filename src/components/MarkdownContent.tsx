import { Check, Copy, FileDiff } from "lucide-react";
import { Children, isValidElement, memo, type ReactNode, useState } from "react";
import ReactMarkdown, { defaultUrlTransform, type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { isDisplayImageSource, MarkdownImage } from "./MarkdownImage";

const PROPOSED_PLAN = /<proposed_plan>([\s\S]*?)(?:<\/proposed_plan>|$)/g;

export const MarkdownContent = memo(function MarkdownContent({ text }: { text: string }) {
  if (!text) return null;

  const parts: ReactNode[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  let key = 0;

  while ((match = PROPOSED_PLAN.exec(text)) !== null) {
    if (match.index > lastIndex) {
      parts.push(<MarkdownDocument text={text.slice(lastIndex, match.index)} key={`markdown-${key++}`} />);
    }
    parts.push(
      <section className="proposed-plan" key={`plan-${key++}`}>
        <div className="proposed-plan-header">
          <FileDiff size={14} aria-hidden="true" />
          <span>提议的计划</span>
        </div>
        <div className="proposed-plan-content">
          <MarkdownDocument text={match[1].trim()} />
        </div>
      </section>,
    );
    lastIndex = PROPOSED_PLAN.lastIndex;
  }

  if (lastIndex < text.length) {
    parts.push(<MarkdownDocument text={text.slice(lastIndex)} key={`markdown-${key}`} />);
  }

  return <div className="markdown-content">{parts.length ? parts : <MarkdownDocument text={text} />}</div>;
});

const MarkdownDocument = memo(
  function MarkdownDocument({ text }: { text: string }) {
    return (
      <ReactMarkdown remarkPlugins={[remarkGfm, remarkArtifactImages]} components={markdownComponents} urlTransform={(url, key, node) => node.tagName === "img" && key === "src" ? (isDisplayImageSource(url) ? url : "") : defaultUrlTransform(url)} skipHtml>
        {text}
      </ReactMarkdown>
    );
  },
  (prev, next) => prev.text === next.text,
);

// Rewrite text nodes only: links, alt text and code retain their original content.
type MarkdownNode = { type: string; value?: string; children?: MarkdownNode[]; url?: string; alt?: string };
function remarkArtifactImages() {
  return (tree: MarkdownNode) => {
    const walk = (node: MarkdownNode) => {
      if (!node.children || ["link", "linkReference", "image", "code", "inlineCode", "html"].includes(node.type)) return;
      node.children = node.children.flatMap(child => {
        if (child.type !== "text" || !child.value) { walk(child); return [child]; }
        const nodes: MarkdownNode[] = [];
        let offset = 0;
        // In a partial stream, an unfinished code span/reference link is still
        // a text node. Do not treat its filename as a standalone artifact.
        for (const match of child.value.matchAll(/(?<![\w/\\.\-`\[\]])\d{10,20}-[0-9a-f]{16,64}\.png\b/gi)) {
          if (match.index > offset) nodes.push({ type: "text", value: child.value.slice(offset, match.index) });
          nodes.push({ type: "image", url: match[0], alt: match[0] });
          offset = match.index + match[0].length;
        }
        if (offset < child.value.length) nodes.push({ type: "text", value: child.value.slice(offset) });
        return nodes;
      });
    };
    walk(tree);
  };
}

const markdownComponents: Components = {
  a: ({ children, ...props }) => (
    <a {...props} target="_blank" rel="noreferrer">
      {children}
    </a>
  ),
  img: ({ src, alt }) => <MarkdownImage source={typeof src === "string" ? src : ""} alt={alt ?? ""} />,
  pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
  table: ({ children, ...props }) => (
    <div className="markdown-table-wrap">
      <table {...props}>{children}</table>
    </div>
  ),
};

function CodeBlock({ children }: { children: ReactNode }) {
  const [copied, setCopied] = useState(false);
  const code = reactNodeText(children).replace(/\n$/, "");

  async function copyCode() {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_500);
    } catch {
      setCopied(false);
    }
  }

  return (
    <div className="markdown-code-block">
      <button type="button" onClick={() => void copyCode()} title="复制代码" aria-label="复制代码">
        {copied ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
      </button>
      <pre>{children}</pre>
    </div>
  );
}

function reactNodeText(node: ReactNode): string {
  return Children.toArray(node)
    .map((child) => {
      if (typeof child === "string" || typeof child === "number") return String(child);
      if (isValidElement<{ children?: ReactNode }>(child)) return reactNodeText(child.props.children);
      return "";
    })
    .join("");
}
