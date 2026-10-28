# Design Plan: Custom Help Output & Meta-repo Clone for `meta git` Plugin

---

## 1. Help Request Detection

- The **main CLI (`meta`)** dispatches commands to plugins.
- When user runs:
  ```
  meta git --help
  meta git clone --help
  ```
- The CLI **detects `--help` or `-h`** in the argument list **before** delegating to system git.
- The CLI **invokes the plugin's `get_help_output(args)` method**.
- The plugin **decides** whether to:
  - **Override** help output completely
  - **Prepend** custom help, then append system git help
  - **Fallback** to default system git help

---

## 2. Plugin API Extension

- Extend `meta_plugin_api` with:
  ```rust
  enum HelpMode {
      Override,
      Prepend,
      None,
  }

  trait MetaPlugin {
      /// Return (HelpMode, help text) if plugin wants to customize help
      fn get_help_output(&self, args: &[String]) -> Option<(HelpMode, String)>;
  }
  ```
- The **main CLI**:
  - Calls `get_help_output(args)`
  - If `Override`, print plugin help only
  - If `Prepend`, print plugin help + then system git help
  - If `None` or `None` returned, fallback to system git help

---

## 3. Help Output Content & Branding

- **Always start plugin help with:**
  ```
  meta git - Meta CLI Git Plugin
  (This is NOT plain git)
  ```
- **Sections:**
  - **About:** What is `meta git`
  - **Meta-repo Commands:** e.g., `meta git clone`
  - **Overrides:** Any commands with special behavior
  - **Examples:** Usage demos
  - **Note:** "For standard git commands, see below" (if prepending)

---

## 4. Meta-repo Clone Command Design

### Command:
```
meta git clone <meta-repo-url> [options]
```

### Behavior:
- Clone the **meta repository** itself
- Parse its **manifest/config** (e.g., `.meta.yaml` file)
- **Iterate child repositories:**
  - Clone each child repo into its specified path
  - Support options:
    - `--recursive` (clone nested meta repos recursively)
    - `--parallel N` (clone N repos concurrently)
    - `--depth N` (shallow clone)
- **Error handling:**
  - Log failures, continue with others
  - Summary at end: success/fail count

### Example Help Snippet:
```
Meta-repo Commands:
  meta git clone <meta-repo-url> [options]
    Clones the meta repository and all child repositories defined in its manifest.

    Options:
      --recursive       Clone nested meta repositories recursively
      --parallel N      Clone up to N repositories in parallel
      --depth N         Create a shallow clone with truncated history

    Examples:
      meta git clone https://github.com/example/meta-repo.git
      meta git clone --parallel 4 --depth 1 https://github.com/example/meta-repo.git
```

---

## 5. User Experience Principles

- **Clarity:** Always indicate this is a **Meta CLI plugin**, not plain git.
- **Separation:** Clearly separate **meta-specific commands** from standard git commands.
- **Guidance:** Provide **examples** for Meta-repo workflows.
- **Fallback:** If plugin help is prepended, user still sees familiar git help below.
- **Consistency:** Use consistent banners and formatting across plugins.

---

## 6. High-Level Flow Diagram

```mermaid
flowchart TD
    A(User runs meta git --help) --> B{Main CLI detects --help?}
    B -- Yes --> C{Is plugin installed?}
    C -- Yes --> D[Call plugin.get_help_output(args)]
    D --> E{HelpMode}
    E -- Override --> F[Show plugin help only]
    E -- Prepend --> G[Show plugin help + system git help]
    E -- None --> H[Show system git help]
    C -- No --> H
    B -- No --> I[Run command normally]
```

---

## 7. Summary

- **Intercept help flags** in CLI dispatcher.
- **Plugins provide custom help** via API, with override or prepend modes.
- **Help output branded** as Meta CLI plugin, not plain git.
- **Meta-repo clone** command clones meta repo + children, with options.
- **Help documents Meta-repo clone** clearly, with examples.
- **User experience** prioritizes clarity, guidance, and familiarity.

This design ensures users get clear, plugin-specific help and understand powerful Meta-repo commands, while maintaining access to standard git help when needed.