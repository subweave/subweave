import { useRef, useState, type ReactNode } from "react";
import content from "virtual:content";

export function Section({
  id,
  n,
  kicker,
  title,
  lede,
  children,
}: {
  id: string;
  n: number;
  kicker: string;
  title: ReactNode;
  lede?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section id={id} className="section" aria-labelledby={`${id}-title`}>
      <div className="wrap">
        <header className="section-head">
          <p className="kicker">
            <span className="kicker-n">{String(n).padStart(2, "0")}</span>
            {kicker}
          </p>
          <h2 id={`${id}-title`}>{title}</h2>
          {lede && <p className="lede">{lede}</p>}
        </header>
        {children}
      </div>
    </section>
  );
}

// One shell command, coloured at build time, with a copy button.
export function Command({ name, label }: { name: string; label: string }) {
  const [copied, setCopied] = useState(false);
  const code = useRef<HTMLElement>(null);
  return (
    <div className="command">
      <pre tabIndex={0} aria-label={label}>
        <span className="prompt" aria-hidden="true">
          ${" "}
        </span>
        <code ref={code} dangerouslySetInnerHTML={{ __html: content.code[`sh:${name}`] }} />
      </pre>
      <button
        type="button"
        className="copy"
        onClick={() => {
          void navigator.clipboard.writeText(code.current?.textContent?.trim() ?? "").then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1600);
          });
        }}
      >
        <span aria-live="polite">{copied ? "Copied" : "Copy"}</span>
      </button>
    </div>
  );
}
