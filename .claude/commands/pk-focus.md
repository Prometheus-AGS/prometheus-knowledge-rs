# /pk-focus — Load Knowledge Base Context

Load a focused mini-knowledge-base for a topic into this session.

**Usage:** `/pk-focus <topic>`

**Examples:**
- `/pk-focus UAR session bus architecture`
- `/pk-focus TurboQuant KV cache implementation`
- `/pk-focus Prometheus Fabric peer data plane`
- `/pk-focus Kaia agent certification`

---

Run the following, then read the output as session context:

```bash
pk focus "$ARGUMENTS"
```

If `pk` is not on PATH:
```bash
~/.prometheus/bin/pk focus "$ARGUMENTS"
```

**What this does:**
Searches the local knowledge base for articles related to `$ARGUMENTS`,
synthesizes them into a dense markdown brief using the local Qwen model,
and returns it for loading into this session context.

**Tip:** Run `/pk-focus` at the start of any complex session to pre-load
relevant architectural context before asking questions or making changes.
