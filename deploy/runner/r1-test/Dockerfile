# R1 container test image (ADR-013): the runner skeleton plus the Python prototype of the supervisor, a stand-in
# runner and a stand-in agent. Test only; never deployed (the production supervisor is Rust, WP3-RUN).
ARG RUNNER_IMAGE=smart-review-runner:skeleton
FROM ${RUNNER_IMAGE}

# A deliberate setuid-root binary: the test proves no-new-privileges keeps it from raising the agent's euid.
RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/* \
    && install -o root -g root -m 4755 /usr/bin/id /usr/local/bin/setuid-id

COPY --chmod=0755 supervisor.py agent.py runner.py entry.sh /opt/r1-test/

# R1_SUPERVISOR=rust runs the checks against the Rust smart-review-supervisor of the base image.
ENTRYPOINT ["/opt/r1-test/entry.sh"]
CMD ["/usr/bin/python3", "/opt/r1-test/runner.py"]
