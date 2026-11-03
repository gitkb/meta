# Meta Discovery Skill

Discover and list projects in the meta workspace.

## List All Projects

```bash
meta projects list
```

Shows all child repos defined in `.meta`.

## Get Project Configuration

The `.meta` file contains the project manifest:

```bash
cat .meta
```

## With JSON Output

```bash
meta --json projects list
```

Useful for programmatic access to project list.

## Filter by Tag

```bash
meta --tag backend projects list
```

## MCP Tools

When using the meta-mcp server:

- `meta_list_projects` - List all projects with tags
- `meta_get_config` - Get the raw .meta configuration
- `meta_get_project_path` - Get absolute path for a project
- `meta_search_code` - Search code patterns across repos
- `meta_get_file_tree` - Get file tree structure
