# Tamper-evident logs: source notes (AL.1)

- **RFC 6962, Certificate Transparency** (https://www.rfc-editor.org/rfc/rfc6962.html), section 2.1:
  the Merkle Tree Hash over SHA-256 with 0x00/0x01 domain separation, the split at the largest power
  of two below the size, PATH (2.1.1, inclusion) and PROOF/SUBPROOF (2.1.2, consistency). "The
  append-only property of each log is technically achieved using Merkle Trees."
- **RFC 9162, Certificate Transparency 2.0** (https://www.rfc-editor.org/rfc/rfc9162.html), sections
  2.1.3.2 and 2.1.4.2: iterative verification of inclusion and consistency proofs for the same tree.
- **Crosby and Wallach, Efficient Data Structures for Tamper-Evident Logging, USENIX Security 2009**
  (https://www.usenix.org/legacy/event/sec09/tech/full_papers/crosby.pdf): logs built on a hash
  chain "require auditors examine every intermediate event between snapshots"; a history tree
  (a Merkle tree) gives logarithmic membership and incremental proofs.
- **transparency-dev/merkle at 75a6539d7b48af0d529236f24487d2814fd1dca7**, testonly/constants.go:
  the reference leaves, subtree hashes and roots T-AL.1.2 checks.
