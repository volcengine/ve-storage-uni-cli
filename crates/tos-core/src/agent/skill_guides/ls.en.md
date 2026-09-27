## Listing and completeness

- Select the backend's supported scope explicitly: a bucket/prefix for ByteTOS, a bucket/prefix or service-level listing for VeTOS, and the relevant instance/space/folder for ADrive. Check the parameter schema before omitting a path.
- Distinguish returned files/objects from common prefixes/folders. A normal listing is not evidence that all descendants were visited; use the supported recursive option when the task requires a full subtree.
- Inspect pagination metadata, truncation, and continuation markers. Continue with the returned token when needed; do not report a partial page's item count as the total.
- When using output for a later write/delete operation, preserve full identifiers and verify the final selected scope. Listing itself does not authorize mutation.
