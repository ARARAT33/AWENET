# Federation join-token security

New `.awenode`, `.awedc`, and `.dgc` invitations receive an independent 256-bit token from the operating system CSPRNG. Tokens are not derived from node IDs, names, timestamps, or other public configuration fields, so regenerating the same invitation produces a different bearer credential.

Validation is fail-closed for the required prefix and hexadecimal entropy. Existing v1 configurations with 128-bit tokens remain readable for migration compatibility; newly generated configurations always use 256-bit tokens. Token format validation is not issuer authorization and does not by itself make a token one-time: replay prevention requires an issuer-side consume/registration flow, which is intentionally a separate protocol feature.

Federation JSON writes use a unique `create_new` temporary file, `0600` Unix permissions, file `sync_all`, atomic rename, and parent-directory sync. A failed write cleans up the temporary file and leaves the previous configuration intact.
