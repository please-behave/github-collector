# Working on the GitHub Collector

- Keep the collector stateless. The ledger owns persistent platform facts.
- Communicate with the ledger directly over the internal network. Use the ledger-owned collector contract.
- Never log credentials, including GitHub tokens and authorization headers.
