import { cn } from "../lib/utils";

function diffLineClass(line: string): string {
  if (line.startsWith("+++") || line.startsWith("---")) return "text-ink font-medium";
  if (line.startsWith("+")) return "bg-pos-soft text-pos";
  if (line.startsWith("-")) return "bg-neg-soft text-neg";
  if (line.startsWith("@@")) return "bg-brand-soft text-brand";
  if (line.startsWith("diff ") || line.startsWith("index ") || line.startsWith("new file")) {
    return "text-ink-faint";
  }
  return "text-ink-dim";
}

/** 统一 diff 文本的着色渲染（带行号）。 */
export function DiffView({ diff }: { diff: string }) {
  const lines = diff.split("\n");
  return (
    <pre className="font-mono text-[11.5px] leading-[1.7]">
      {lines.map((line, index) => (
        <div key={index} className={cn("flex", diffLineClass(line))}>
          <span className="sticky left-0 w-11 shrink-0 select-none border-r border-line bg-sunken px-2 text-right text-[10px] text-ink-faint">
            {index + 1}
          </span>
          <span className="whitespace-pre px-3">{line || " "}</span>
        </div>
      ))}
    </pre>
  );
}

/** 纯文本文件内容渲染（带行号）。 */
export function FileViewer({ content }: { content: string }) {
  const lines = content.replace(/\n$/, "").split("\n");
  return (
    <pre className="font-mono text-[11.5px] leading-[1.7] text-ink">
      {lines.map((line, index) => (
        <div key={index} className="flex hover:bg-hover">
          <span className="sticky left-0 w-11 shrink-0 select-none border-r border-line bg-sunken px-2 text-right text-[10px] text-ink-faint">
            {index + 1}
          </span>
          <span className="whitespace-pre px-3">{line || " "}</span>
        </div>
      ))}
    </pre>
  );
}
