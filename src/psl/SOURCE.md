# Public suffix list

`public_suffix_list.dat` is the [Public Suffix List](https://publicsuffix.org/),
vendored unchanged (MPL 2.0, its license header kept) for the scope guardrails
(#83): `*.<public suffix>` covers every site registered under it, so spall
refuses it as a scope entry without an audited override.

- Version: `2026-11-24_18-53-27_UTC`, commit `cb13e39ae72a79e17022b313f510c585751b5334`
- SHA-256: `cd1866797f7e9707269cabaf71e46f522299efadd504cdcd429af486bb620de2`

Refresh (only from publicsuffix.org, as the list asks), then update this file
and run `cargo test psl`:

    curl -sSfL https://publicsuffix.org/list/public_suffix_list.dat -o src/psl/public_suffix_list.dat
    sha256sum src/psl/public_suffix_list.dat
