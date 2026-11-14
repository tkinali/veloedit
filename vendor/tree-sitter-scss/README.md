# tree-sitter-scss (VeloEdit's copy)

This is `tree-sitter-scss` **1.0.0** from crates.io
(<https://github.com/tree-sitter-grammars/tree-sitter-scss>, MIT license, by Amaan Qureshi) with
**one change**, in `bindings/rust/build.rs`:

```diff
-    c_config.flag("-Wno-unused-parameter");
+    c_config.flag_if_supported("-Wno-unused-parameter");
```

The original hands that GCC/Clang option to every compiler. MSVC's `cl.exe` rejects it
(`D8021: invalid numeric argument '/Wno-unused-parameter'`), so the crate cannot be built on
Windows with Visual Studio. `flag_if_supported` is what the other tree-sitter grammar crates use.

Only the files needed to build are kept (no `grammar.js`, `grammar.json`, README of the original).
It is used through `[patch.crates-io]` in the root `Cargo.toml`.

**Remove this directory and the patch entry** as soon as a `tree-sitter-scss` release builds with MSVC.
