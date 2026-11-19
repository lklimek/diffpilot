# R1 container test image (ADR-013): the runner image (target `base`, stripped here like production but keeping
# python3) plus the Python prototype of the supervisor, a stand-in runner and a stand-in agent. Test only; never
# deployed.
ARG RUNNER_IMAGE=smart-review-runner:base
FROM ${RUNNER_IMAGE}

# A deliberate setuid-root binary: the test proves no-new-privileges keeps it from raising the agent's euid.
# apt's sandbox user _apt cannot use the base image's /tmp (runner-only, 0700), so this step uses /var/tmp.
RUN export TMPDIR=/var/tmp \
    && apt-get update \
    && apt-get install -y --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/* \
    && install -o root -g root -m 4755 /usr/bin/id /usr/local/bin/setuid-id \
    && /usr/local/lib/smart-review/strip-image.sh python3

COPY --chmod=0755 supervisor.py agent.py runner.py entry.sh /opt/r1-test/

# R1_SUPERVISOR=rust runs the checks against the Rust smart-review-supervisor of the base image.
ENTRYPOINT ["/opt/r1-test/entry.sh"]
CMD ["/usr/bin/python3", "/opt/r1-test/runner.py"]
