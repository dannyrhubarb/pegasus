# Staging preview smoke test

Scratch branch: exists only to spin up a PR preview built against the
staging backend (BACKEND_CONFIG_JSON_STAGING, pegasus#171). Close the PR
when done — preview-teardown cleans up the deployment.
