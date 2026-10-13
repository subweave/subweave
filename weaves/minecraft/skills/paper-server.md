# Paper server

Downloads come from the Fill API:

```
curl -s https://fill.papermc.io/v3/projects/paper/versions/{version}/builds/latest \
  | jq '{id, url: .downloads."server:default".url, sha256: .downloads."server:default".checksums.sha256}'
curl -s https://fill.papermc.io/v3/projects/paper/versions/{version} \
  | jq -r '.version.java.version.minimum, (.version.java.flags.recommended | join(" "))'
```

The second command gives the minimum Java release and Paper's recommended JVM flags. Add `-Xms` and `-Xmx` at about 70% of the box's memory.

`server.properties`:

| Key | Value |
|---|---|
| `max-players` | `{players}` |
| `gamemode` | `{mode}` |
| `white-list`, `enforce-whitelist` | `true` for `whitelist` |
| `enable-rcon` | `true` |
| `rcon.port` | `25575` |
| `rcon.password` | from `$MCRCON_PASS`; never echo it |

RCON listens on every address. Allow `25565/tcp` in ufw and nothing for `25575`.

mcrcon is not in Ubuntu's packages: install the release binary from github.com/Tiiffi/mcrcon. It reads `MCRCON_PASS`, `MCRCON_HOST` and `MCRCON_PORT`; the defaults are localhost and 25575. `-c` turns off colour codes.

Accept the EULA by writing `eula=true` to `/opt/minecraft/eula.txt`.
