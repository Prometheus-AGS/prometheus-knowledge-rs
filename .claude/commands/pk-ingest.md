# /pk-ingest — Save Notes to Knowledge Base

Compile and save content into the persistent knowledge base.

**Usage:** `/pk-ingest [file]` or pipe via stdin

**Examples:**
- `/pk-ingest` — ingests the current session summary
- `/pk-ingest notes.md` — ingests a specific file
- `/pk-ingest --source "session:uar-refactor-2026-04-10"` — with a source label

---

```bash
# Ingest a file
pk ingest "$ARGUMENTS"

# Or ingest the current session context summary (no arguments)
# Claude Code will prompt you to provide content to save
```

**What this does:**
The `pk` CLI sends the content to the Librarian, which calls the compile
LLM (cloud model) to structure it into a wiki article with tags, links,
and frontmatter, then saves it to `~/.prometheus/knowledge/wiki/`.

**PMPO Reflect integration:**
Add to your session end workflow:
```bash
echo "$SESSION_NOTES" | pk ingest --source "pmpo:reflect:$(date +%s)"
```
