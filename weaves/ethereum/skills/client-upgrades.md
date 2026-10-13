# Upgrading reth and lighthouse

Release assets for this box: `reth-<tag>-x86_64-unknown-linux-gnu.tar.gz` and `lighthouse-<tag>-x86_64-unknown-linux-gnu.tar.gz`, each with a `.asc` signature.

1. Download the tarball and its `.asc`. Verify with `gpg --verify`; import the signing key from the project's release notes if it is missing.
2. Copy the running binary to `/opt/eth/previous/`.
3. Install the new binary to `/usr/local/bin/` with `install -m 755`.
4. Restart only that client. Check that the head moves within 15 minutes.

Read the release notes before step 2:

- "database migration", "breaking", "re-sync" or "schema": the weave's `ask` rule applies; stop and let the person answer.
- "mandatory" or a fork name with a date: the release is needed before that date.
