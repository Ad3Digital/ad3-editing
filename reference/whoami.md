# `dapi whoami`

Prints the authenticated account, or `null` if signed out.

## Input

None.

## Output

One JSON value:

```ts
{ id: string; email: string; provider: string } | null
```
