import content from "virtual:content";

// Code coloured at build time (build/shiki.ts): Weavefiles by the repo's grammar, shell and JSON by Shiki's.
export function Code({
  name,
  label,
  numbered = false,
  className = "",
}: {
  name: string;
  label: string;
  numbered?: boolean;
  className?: string;
}) {
  const html = content.code[name];
  if (html === undefined) throw new Error(`no code named ${name}`);
  return (
    <pre className={`code-block${numbered ? " numbered" : ""} ${className}`} tabIndex={0} aria-label={label}>
      <code dangerouslySetInnerHTML={{ __html: html }} />
    </pre>
  );
}
