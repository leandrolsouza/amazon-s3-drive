# Instruções do backend Tauri

Estas regras complementam as instruções de [`../AGENTS.md`](../AGENTS.md) e se aplicam a este diretório.

## Arquitetura

- `src/main.rs` inicia o aplicativo; `src/lib.rs` registra o estado, comandos Tauri e módulos do núcleo.
- Mantenha responsabilidades nos módulos existentes (`sync_engine`, `s3_client`, `db`, `file_watcher`, `queue_manager`, `credentials_manager` e demais módulos), evitando concentrar lógica nova em `lib.rs`.
- Comandos invocados pela interface e eventos enviados ao frontend são contratos externos ao módulo. Ao alterá-los, atualize os usos correspondentes em `../src/` e os tipos compartilhados.
- Use os tipos serializáveis em `models.rs` como contrato de dados quando aplicável; preserve nomes e formatos que já são consumidos pelo frontend.

## Segurança e sincronização

- Nunca registre credenciais ou tokens AWS. Use `CredentialsManager` e o armazenamento seguro do sistema para segredos.
- Trate nomes de bucket, prefixos e caminhos locais como entrada não confiável; valide antes de acessar arquivos ou objetos S3.
- Preserve a segurança de operações destrutivas: respeite exclusões, conflitos, cancelamento, fila e estado persistido. Não introduza remoções remotas ou locais implícitas.
- Mantenha operações de rede e sistema de arquivos assíncronas quando apropriado e evite bloquear o runtime Tokio.
- Ao alterar esquemas ou persistência SQLite, considere bancos já existentes e implemente migração compatível quando necessária.
- Mantenha erros úteis para a interface sem incluir segredos ou dados sensíveis.

## Verificação

- Use `cargo fmt --manifest-path src-tauri/Cargo.toml` para formatar Rust.
- Use `cargo test --manifest-path src-tauri/Cargo.toml` para executar a suíte Rust, incluindo testes de integração.
- Se o contrato de comandos, eventos ou modelos mudar, verifique também o frontend e seus testes.
