## What does this change?

## Why?

## Checklist

- [ ] I have read [`CLA.md`](../CLA.md) and accept it for this contribution.
      Opening this pull request is acceptance either way; ticking it says you
      read it first.
- [ ] `npm --prefix ui run check`
- [ ] `cargo test --locked`
- [ ] `cargo clippy --locked --all-targets -- -D warnings`
- [ ] `cargo deny check`
- [ ] New texts are in every language, not only English
- [ ] A new dependency: `npm run notices`, and `THIRD-PARTY-NOTICES.txt` committed
