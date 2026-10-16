// Shell the page shows, coloured at build time like the Weavefiles.
// Commands as WEAVEFILE.md and the README write them.
export const SHELL: Record<string, string> = {
  run: `subweave run postgres "postgres 18, private network, backups to s3://acme-pg/db1"`,
  install: "curl -fsSL https://subweave.com/install.sh | sh",
  learned: `subweave learned db              # the commands this box learned
subweave forget db free-logs     # drop one
subweave learn db                # draft SKILL recipes and KNOWN lines for the weave's author`,
  channels: `subweave channel add steery                        # a Steery key, read from stdin
subweave channel add telegram 123456789            # the chat id; the bot token from stdin
subweave channel add slack C0123ABCD               # the channel id; a bot token from stdin
subweave channel add email ops@acme.dev            # an SMTP URL, from stdin
subweave channel add webhook https://ops.acme.dev/subweave
subweave channel add heartbeat https://hc-ping.com/<uuid>`,
  first: `subweave key openrouter
subweave plan postgres "postgres 18, private network, backups to s3://acme-pg/db1"
subweave run box --on root@db1
subweave run postgres "postgres 18, private network, backups to s3://acme-pg/db1" --on root@db1`,
};

// A Dockerfile as written in August 2024, for "What about costs?": postgres:16.4 was the
// newest image, and wal-g v3.0.3 the newest release with this file name.
export const DOCKERFILE = `FROM postgres:16.4-bookworm
ARG WALG=https://github.com/wal-g/wal-g/releases/download/v3.0.3
ADD $WALG/wal-g-pg-ubuntu-20.04-amd64.tar.gz /tmp/wal-g.tgz
RUN tar -xzf /tmp/wal-g.tgz -C /usr/local/bin`;
