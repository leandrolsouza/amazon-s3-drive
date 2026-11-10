# Instruções do repositório

## Projeto

Aplicativo desktop de sincronização bidirecional entre uma pasta local e Amazon S3, feito com React, TypeScript, Vite e Tauri 2. A interface fica em `src/`; o núcleo Rust e a integração com o sistema ficam em `src-tauri/`. Consulte `.kiro/specs/amazon-s3-drive/` para requisitos, desenho e tarefas do produto.

## Convenções de trabalho

- Preserve o escopo desktop e a compatibilidade com os sistemas declarados em `src-tauri/tauri.conf.json`.
- Mantenha os tipos e nomes de comandos/eventos usados na ponte frontend-backend consistentes nos dois lados.
- Não inclua credenciais, tokens, dados locais de usuário ou segredos em código, logs, testes ou documentação. Credenciais AWS devem continuar no gerenciador seguro do sistema.
- Prefira mudanças pequenas que respeitem os padrões existentes. Não reformate arquivos ou altere dependências sem necessidade para a tarefa.
- Ao mudar comportamento ou arquitetura, atualize os documentos correspondentes em `.kiro/specs/amazon-s3-drive/` quando necessário.

## Comandos úteis

- `npm run dev`: inicia o frontend Vite.
- `npm run build`: verifica tipos TypeScript e gera o frontend de produção.
- `npm test`: executa os testes frontend com Vitest.
- `cargo test --manifest-path src-tauri/Cargo.toml`: executa os testes Rust.
- `npm run tauri dev`: inicia o aplicativo desktop em desenvolvimento.

Execute verificações relevantes para as alterações feitas e informe claramente o que foi ou não executado.
