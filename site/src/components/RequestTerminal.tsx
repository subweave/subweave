import { useEffect, useRef, useState, type ReactNode } from "react";
import content from "virtual:content";
import { useReducedMotion } from "../motion";
import { COMMAND, heroFindings } from "../weave/example";

const eth = content.examples.find((w) => w.name === "ethereum")!;
const { findings, ask } = heroFindings(eth);

const LINES: { cls: string; node: ReactNode; ms: number }[] = [
  ...findings.map(([level, subject, text]) => ({
    cls: `finding level-${level}`,
    node: (
      <>
        <span className="f-level">{level}</span>
        <span className="f-subject">{subject}</span>
        <span className="f-text">{text}</span>
      </>
    ),
    ms: 260,
  })),
  { cls: "t-line t-gap", node: "1 question: run asks the person first.", ms: 900 },
  { cls: "t-line t-ask", node: `asked #1 on steery: ASK IF ${ask}`, ms: 1400 },
  { cls: "t-line t-yes", node: "#1 yes, signed on the person's device", ms: 700 },
  { cls: "t-line t-dim", node: "the daemon starts: installed, running, synced …", ms: 4000 },
];

// `subweave run ethereum` with too little history: the decider fills `blocks`, code compares, the person answers.
export function RequestTerminal() {
  const reduced = useReducedMotion();
  // Full output until the terminal is first seen; then it replays in a loop.
  const [typed, setTyped] = useState(COMMAND.length);
  const [shown, setShown] = useState(LINES.length);
  const [started, setStarted] = useState(false);
  const [visible, setVisible] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const io = new IntersectionObserver(([e]) => setVisible(e.isIntersecting), { threshold: 0.3 });
    io.observe(el);
    return () => io.disconnect();
  }, []);

  useEffect(() => {
    if (reduced || !visible) return;
    if (!started) {
      setStarted(true);
      setTyped(0);
      setShown(0);
      return;
    }
    if (typed < COMMAND.length) {
      const id = setTimeout(() => setTyped((x) => x + 1), typed === 0 ? 500 : 14);
      return () => clearTimeout(id);
    }
    const id = setTimeout(
      () => {
        if (shown < LINES.length) setShown((x) => x + 1);
        else {
          setTyped(0);
          setShown(0);
        }
      },
      shown === 0 ? 400 : LINES[shown - 1].ms,
    );
    return () => clearTimeout(id);
  }, [typed, shown, reduced, visible, started]);

  const t = reduced ? COMMAND.length : typed;
  const s = reduced ? LINES.length : shown;
  return (
    <div className="screen" ref={ref}>
      <div className="screen-bar">
        <span>terminal</span>
      </div>
      <div className="screen-log" aria-hidden="true">
        <p className="cmdline">
          <span className="prompt">$ </span>
          {COMMAND.slice(0, t)}
          {t < COMMAND.length && <span className="caret" />}
        </p>
        {LINES.map((l, i) => (
          <p key={i} className={`${l.cls}${i < s ? "" : " hidden"}`}>
            {l.node}
          </p>
        ))}
      </div>
      <p className="sr-only">
        {COMMAND}. The check finds: {findings.map((f) => f.join(" ")).join("; ")}. The person answers yes on Steery.
      </p>
    </div>
  );
}
