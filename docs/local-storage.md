# Local storage

On first launch, Heya asks whether to encrypt its local data. Encryption is
selected by default. Existing profiles keep their encryption without another
setup prompt. Change the choice with **Settings → System → Encrypt local data**.

- **Encrypted:** the protocol session, chats, drafts, outbox and cached media
  use SQLCipher. Database keys are derived separately from one random master key
  stored in the OS key store: Secret Service on Linux, Keychain on macOS, or the
  Windows credential store. The OS may ask you to unlock that store when Heya
  connects or opens encrypted saved chats. A missing key cannot be bypassed by
  turning encryption off; decryption needs the original key.
- **Unencrypted:** both databases are ordinary SQLite files. Heya does not read,
  write or unlock the key store in this mode. Anyone who can read the data folder
  can read conversations and copy the saved credentials. Private folder/file
  permissions are retained. Full-disk encryption and OS account security remain
  useful, but are independent of this choice.

Messages on the WhatsApp connection continue to use the protocol's encryption
in either mode. This setting concerns Heya's active local profile only. Files
saved in Downloads, backups, snapshots and other copies are not converted or
securely erased. An unused master-key entry may remain in the key store after
turning encryption off; unencrypted startup does not depend on it.

When the OS key store has not started yet, automatic connection waits and retries
after 2, 4, 8, 16 and then 30 seconds (capped at 30 seconds). Each blocking attempt
finishes before another begins; only the existing async lifecycle task sleeps
between attempts. Cancel connection/window close stops that wait. A dismissed
unlock prompt, denied access, missing key, malformed key or database error is
terminal and requires an explicit retry; plaintext mode bypasses this entirely.

## Changing modes

Confirm **Restart Heya**. Finish or cancel an active call, voice recording
or attachment first. Heya uses its normal shutdown to save drafts and release
the protocol and archive, then restarts with a conversion progress dialog.
Conversion needs free space for another copy of both databases. No messages are
sent and no new WhatsApp link is needed for the conversion itself.

The stable profile lock covers both databases and conversion. An unconfigured
profile makes its choice without opening a database or accessing the key store.
For existing data, conversion exports each database with SQLCipher, preserves
schema versions, checks database integrity and syncs the replacements. One
atomic `storage.json` manifest replacement selects their storage mode and data
directory together. The manifest contains no keys or account data.

The original files remain active until both replacements are ready. Failed
exports remove their staging directory and leave the active databases intact.
After a successful commit, old database files and journals are removed. Startup
removes incomplete or obsolete conversion directories left by interruption
before opening data. If cleanup fails, Heya reports it and refuses to open the
profile until cleanup can finish. Never delete the active files or change the
manifest by hand to recover a key or conversion error.

Synthetic tests cover ordinary SQLite startup without a functioning key store,
both conversion directions, legacy profiles, real archive reopen, drafts, queued
sends, media bytes, interrupted exports before/after manifest publication, corrupt
metadata, missing keys and exclusive ownership. Toolkit tests cover first-run
choice, delayed/failed acceptance, duplicate submissions, busy activity rejection
and narrow layouts. These do not establish real OS key-store or live-account
compatibility on another machine.

On Linux, `scripts/ui-offscreen.sh --storage-check /tmp/heya-storage` also checks
the native first-run choice, plaintext archive opening without a session bus/key
store, and preservation of both databases after a failed encryption attempt.
It saves synthetic screenshots for visual inspection. All data is disposable:
archive/conversion cases use `--offline`, and automatic startup uses a synthetic
session with an unavailable test bus. No real account is opened or messaged.
