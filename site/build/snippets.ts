// Shell the page shows, coloured at build time like the Weavefiles.
// Commands as WEAVEFILE.md and the README write them.
export const SHELL: Record<string, string> = {
  run: `subweave run postgres "postgres 18, private network, backups to s3://acme-pg/db1"`,
  install: "curl -fsSL https://subweave.com/install.sh | sh",
  learned: `subweave learned db              # the commands this box learned
subweave forget db free-disk     # drop one
subweave learn db                # draft SKILL recipes for the weave's author`,
  channels: `subweave channel add steery                        # a Steery key, read from stdin
subweave channel add telegram 123456789            # the chat id; the bot token from stdin
subweave channel add slack                         # an incoming-webhook URL, from stdin
subweave channel add email ops@acme.dev            # an SMTP URL, from stdin
subweave channel add webhook https://ops.acme.dev/subweave
subweave channel add heartbeat https://hc-ping.com/<uuid>`,
  models: `subweave key openrouter                  # one key serves both models
subweave run postgres "postgres 18"

subweave provider add gpu1 http://10.0.0.7:8000/v1     # a vLLM box; the protocol defaults to openai
subweave run postgres "postgres 18" --code Qwen/Qwen3-Coder-30B-A3B-Instruct@gpu1`,
  first: `subweave key openrouter
subweave plan postgres "postgres 18, private network, backups to s3://acme-pg/db1"
subweave run postgres "postgres 18, private network, backups to s3://acme-pg/db1" --on root@db1`,
};
