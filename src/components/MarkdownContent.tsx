import { Check, Copy, FileDiff } from "lucide-react";
import { Children, isValidElement, type ReactNode, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

const PROPOSED_PLAN = /<proposed_plan>([\s\S]*?)(?:<\/proposed_plan>|$)/g;

export function MarkdownContent({ text }: { text: string }) {
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
}

function MarkdownDocument({ text }: { text: string }) {
  return (
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents} skipHtml>
      {text}
    </ReactMarkdown>
  );
}

const markdownComponents: Components = {
  a: ({ children, ...props }) => (
    <a {...props} target="_blank" rel="noreferrer">
      {children}
    </a>
  ),
  img: ({ alt }) => <span className="markdown-image-placeholder">{alt || "图片"}</span>,
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
