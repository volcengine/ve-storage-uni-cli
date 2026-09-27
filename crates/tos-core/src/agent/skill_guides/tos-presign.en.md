## Signed URL delivery

ZTI does not support presign. This command requires `aksk`; do not switch
authentication mode without the user's authorization.

- Select the exact object, HTTP method, and lifetime. The default method is GET; PUT authorizes upload to the signed target, HEAD reads metadata, and DELETE authorizes deletion. Confirm the requested action before choosing a write/delete method. Use the shortest lifetime that satisfies the request.
- Creating a signature does not prove that the object exists or that a later request will succeed. Verify the target separately when existence is required.
- The URL conveys temporary access. Return the complete usable URL to the requesting user when they explicitly asked for it, but do not put it in diagnostic logs or unrelated artifacts. Preserve its query string and do not alter the signed path or method.
