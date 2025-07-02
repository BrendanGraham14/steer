# JSON Schema for Conductor Session Configuration

This directory contains the JSON Schema for Conductor session configuration files.

## Usage

The schema is automatically linked in our example configuration files via the `$schema` key. Modern editors like VS Code with the "Even Better TOML" extension will use this schema to provide:

- **Autocomplete**: Suggesting valid configuration keys
- **Validation**: Highlighting errors for invalid keys or values  
- **Documentation**: Showing descriptions on hover

## Regenerating the Schema

The schema is auto-generated from our Rust code. To regenerate it after making changes to the configuration structures:

```bash
cargo run -p conductor-cli --bin schema-generator
```

This will write the updated schema to `schemas/session.schema.json`.

## Contributing to SchemaStore

Once the project is stable, we should consider contributing this schema to [SchemaStore](https://www.schemastore.org/json/) so that editors can automatically provide schema support for any file named `session.toml` without requiring the `$schema` key.