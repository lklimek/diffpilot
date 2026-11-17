#!/bin/sh
# R1 container test entrypoint: the Python prototype supervisor (default) or, with R1_SUPERVISOR=rust, the Rust
# smart-review-supervisor of the runner image; both serve the runner command given as arguments.
if [ "${R1_SUPERVISOR:-python}" = rust ]; then
  exec /usr/local/bin/smart-review-supervisor serve -- "$@"
fi
exec /usr/bin/python3 /opt/r1-test/supervisor.py serve "$@"
