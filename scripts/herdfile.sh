#!/bin/sh
# Run the plugin's own herdfile binary if the build put one in ./bin, else the
# one on PATH.
here=$(cd "$(dirname "$0")/.." && pwd)
if [ -x "$here/bin/herdfile" ]; then
  exec "$here/bin/herdfile" "$@"
fi
exec herdfile "$@"
