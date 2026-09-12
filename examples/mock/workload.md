# Mock SHA-256 fixture v1

Read the canonical input as exact bytes, with no decoding or preprocessing.
Compute SHA-256 and output its 64 lowercase hexadecimal ASCII digits followed
by one LF byte. Commit to the input and canonical output with SHA-256.

The mock trace and proof retain the input, so this fixture provides no privacy.
Its hash-sealed artifacts have no proof-system security. Use it to exercise
orchestration and failure handling, never to compare real zkVM performance.
