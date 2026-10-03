# 9. Secrets

Files with passwords or tokens, like `rclone.conf`, can live in the repo encrypted, so it can be public:

```sh
maw add --secret ~/.config/rclone/rclone.conf   # encrypted into static/rclone/rclone.conf.age
maw secret edit ~/.config/rclone/rclone.conf    # decrypted into your editor, encrypted back
maw secret rekey                                # re-encrypted for every machine, after adding one
```

```
record hosts/laptop.pub
encrypt ~/.config/rclone/rclone.conf -> static/rclone/rclone.conf.age
```

Files are encrypted with [age](https://age-encryption.org) to your SSH key: `~/.ssh/id_ed25519`, else `~/.ssh/id_rsa`, or `maw.secretKey = "~/.ssh/other";` in `config.nix`. Each machine's public key is in the repo as `hosts/<name>.pub` (written the first time a machine encrypts), and every file is encrypted to all of them, so each machine decrypts with its own key and no private key leaves its machine. It needs `age` (`maw install age`); maw itself doesn't depend on it.

On activation, an encrypted file is decrypted into `~/.local/state/maw/secrets/` (readable only by you) and the live file links to that copy; the plaintext never enters the repo. It's decrypted again only when the encrypted file changes, so a key with a passphrase is asked for rarely; `status`, `diff`, and dry runs never decrypt. If decrypting fails (a mistyped passphrase, age missing), the last decrypted copy stays linked and activation notes why; it's tried again next time. Edits made through the live file stay until the encrypted file next changes; then the edited copy is moved to the backups dir, and activation says where. `maw secret edit` is how an edit reaches the repo.

A new machine can't decrypt anything until it's a recipient. Its first activation skips encrypted files with a note (`skip static/rclone/rclone.conf.age: ... isn't encrypted for this machine`). To add it: run `maw secret rekey` there, which records its key as `hosts/<name>.pub` (and warns about each file it can't open yet), push; then `maw secret rekey` and push on a machine that can decrypt, and pull on the new one.

Next: chapter 10, themes (`maw help 10`).
