# Requirements Document

## Reconciliação de exclusões (complemento)

- Em modo bidirecional, comparar o registro da última sincronização com o estado local e a listagem S3, inclusive após reinicialização. Ausência de arquivo conhecido representa uma exclusão pendente, não um novo download.
- Exclusões detectadas pelo monitor ou pela reconciliação exigem confirmação. Oferecer restauração e confirmação individual; permitir confirmação em lote apenas para exclusões sem conflito.
- Exclusão de um lado e edição do outro constituem conflito. Preservar a versão existente até uma decisão explícita; revalidar a revisão apresentada e os conteúdos antes de executar.
- Persistir pendências entre sessões. Falhas mantêm a pendência e o histórico para nova tentativa. Se ambos os lados já foram excluídos, limpar o registro.
- Antes de propagar uma exclusão, preservar a versão restante em `.s3-drive-trash` no lado de destino. Essas cópias não são sincronizadas e não têm expiração automática; a recuperação é manual. Operações explícitas de exclusão pela tela de arquivos mantêm seu fluxo próprio de confirmação.
- Pasta raiz indisponível e erros de leitura ou listagem não devem ser interpretados como exclusão. Respeitar padrões de exclusão e os modos UploadOnly/DownloadOnly.

## Introduction

O Amazon S3 Drive é uma aplicação desktop que cria um drive virtual no computador do usuário, conectado a um bucket Amazon S3. Inspirado em aplicações como o Google Drive, o aplicativo sincroniza arquivos de forma bidirecional entre o sistema de arquivos local e o S3, permitindo acesso transparente aos arquivos como se fossem pastas locais do sistema operacional. O aplicativo roda em segundo plano, monitora alterações em ambos os lados e garante consistência dos dados com suporte a operações offline e resolução de conflitos.

## Glossary

- **S3_Drive**: O aplicativo principal que gerencia o drive virtual conectado ao Amazon S3
- **S3_Bucket**: O contêiner de armazenamento no Amazon S3 associado ao drive
- **Pasta_Local**: O diretório local no computador do usuário mapeado ao S3_Bucket
- **Sync_Engine**: O componente responsável pela sincronização bidirecional de arquivos entre a Pasta_Local e o S3_Bucket
- **File_Watcher**: O componente que monitora alterações no sistema de arquivos local em tempo real
- **S3_Client**: O componente que se comunica com a API do Amazon S3
- **Credentials_Manager**: O componente que gerencia e armazena as credenciais de autenticação AWS
- **Conflict_Resolver**: O componente que detecta e resolve conflitos entre versões de arquivos modificados em ambos os lados
- **Config_Parser**: O componente que lê e escreve o arquivo de configuração da aplicação
- **Status_Monitor**: O componente que rastreia e exibe o estado de sincronização dos arquivos
- **Notification_Service**: O componente que envia notificações ao usuário sobre eventos de sincronização
- **Queue_Manager**: O componente que enfileira e processa operações pendentes de sincronização

## Requirements

### Requisito 1: Montagem do Drive Local

**User Story:** Como usuário, quero acessar meus arquivos S3 através de uma pasta local, para que eu possa interagir com os arquivos do S3 como se estivessem no meu computador.

#### Critérios de Aceitação

1. WHEN o S3_Drive é iniciado pela primeira vez, THE S3_Drive SHALL criar uma Pasta_Local dedicada no caminho configurado pelo usuário, caso ela não exista, mapeada ao S3_Bucket
2. WHEN o S3_Drive é iniciado, THE S3_Drive SHALL listar todos os objetos e prefixos do S3_Bucket e representá-los como entradas de arquivo e diretório na Pasta_Local
3. WHEN um usuário abre a Pasta_Local, THE S3_Drive SHALL exibir todos os arquivos e diretórios armazenados no S3_Bucket em até 3 segundos
4. THE S3_Drive SHALL preservar a estrutura hierárquica de diretórios do S3_Bucket na Pasta_Local, utilizando prefixos S3 como separadores de diretório
5. WHEN o S3_Drive é encerrado, THE S3_Drive SHALL completar todas as transferências em andamento ou aguardar no máximo 60 segundos antes de cancelar as transferências pendentes e finalizar o processo
6. IF o caminho configurado para a Pasta_Local for inválido ou inacessível, THEN THE S3_Drive SHALL exibir uma mensagem de erro descritiva e não iniciar a sincronização
7. IF o S3_Bucket configurado estiver inacessível durante a inicialização, THEN THE S3_Drive SHALL exibir uma mensagem de erro descritiva e aguardar nova tentativa manual do usuário

---

### Requisito 2: Sincronização de Upload

**User Story:** Como usuário, quero que os arquivos adicionados ou modificados na pasta local sejam automaticamente enviados ao S3, para que meus arquivos sejam armazenados na nuvem sem intervenção manual.

#### Critérios de Aceitação

1. WHEN um arquivo é criado na Pasta_Local, THE Sync_Engine SHALL enviar o arquivo para o caminho correspondente no S3_Bucket em até 30 segundos após a detecção
2. WHEN um arquivo é modificado na Pasta_Local, THE Sync_Engine SHALL enviar o arquivo atualizado para o S3_Bucket em até 30 segundos após a conclusão da escrita
3. WHEN um arquivo é renomeado na Pasta_Local, THE Sync_Engine SHALL copiar o arquivo para o novo caminho no S3_Bucket e excluir o objeto original em até 30 segundos
4. WHEN um arquivo é movido entre diretórios na Pasta_Local, THE Sync_Engine SHALL copiar o objeto para o novo caminho no S3_Bucket e excluir o objeto original em até 30 segundos
5. WHEN um arquivo conhecido é excluído da Pasta_Local em modo bidirecional, THE Sync_Engine SHALL apresentar uma revisão persistida para restaurar o arquivo ou confirmar a exclusão no S3_Bucket, preservando uma cópia para recuperação antes da remoção
6. WHEN um arquivo maior que 100MB é enviado, THE S3_Client SHALL utilizar multipart upload com partes de no mínimo 5MB cada, exceto a última parte
7. IF uma transferência de upload é interrompida, THEN THE Sync_Engine SHALL retomar o upload a partir da última parte concluída com sucesso em até 3 tentativas, mantendo o estado de retomada por no máximo 24 horas
8. WHEN um upload falha após 3 tentativas consecutivas, THE Sync_Engine SHALL marcar o arquivo com estado `erro` e notificar o usuário com o nome do arquivo e a descrição do erro
9. IF a exclusão do objeto original no S3_Bucket falhar após uma operação de renomeação ou movimentação bem-sucedida, THEN THE Sync_Engine SHALL preservar ambos os objetos no S3_Bucket, marcar a operação como `erro` e notificar o usuário

---

### Requisito 3: Sincronização de Download

**User Story:** Como usuário, quero que os arquivos adicionados ou modificados no S3 sejam automaticamente baixados para a pasta local, para que eu sempre tenha a versão mais recente dos arquivos.

#### Critérios de Aceitação

1. WHEN um objeto é criado no S3_Bucket por uma fonte externa, THE Sync_Engine SHALL baixar o arquivo para a Pasta_Local em até 60 segundos após a detecção, substituindo qualquer versão local existente
2. WHEN um objeto é modificado no S3_Bucket por uma fonte externa, THE Sync_Engine SHALL baixar a versão atualizada para a Pasta_Local em até 60 segundos após a detecção, substituindo o arquivo local existente
3. WHEN um objeto conhecido é excluído do S3_Bucket por uma fonte externa em modo bidirecional, THE Sync_Engine SHALL apresentar uma revisão persistida para restaurar o objeto ou confirmar a remoção local, sinalizando conflito se o conteúdo local mudou e preservando uma cópia antes de remover
4. THE Sync_Engine SHALL verificar alterações no S3_Bucket em intervalos de no máximo 60 segundos utilizando polling da API S3
5. IF um download é interrompido, THEN THE Sync_Engine SHALL retomar o download em até 3 tentativas com intervalo de 10 segundos entre cada tentativa
6. WHEN um download é concluído, THE Sync_Engine SHALL validar a integridade do arquivo baixado comparando o checksum MD5 com o ETag do objeto S3
7. IF a validação de checksum falhar após o download, THEN THE Sync_Engine SHALL descartar o arquivo baixado e registrar o erro de sincronização com o nome do arquivo, o checksum esperado e o checksum recebido
8. IF todas as tentativas de download falharem, THEN THE Sync_Engine SHALL preservar a versão local existente do arquivo e registrar o erro de sincronização com o nome do arquivo, número de tentativas e descrição do erro

---

### Requisito 4: Autenticação e Credenciais AWS

**User Story:** Como usuário, quero configurar minhas credenciais AWS de forma segura, para que o aplicativo possa acessar meu bucket S3 com autenticação adequada.

#### Critérios de Aceitação

1. THE Credentials_Manager SHALL aceitar Access Key ID (20 caracteres alfanuméricos maiúsculos), Secret Access Key (40 caracteres) e AWS Region (identificador de região válido como "us-east-1") como parâmetros de autenticação obrigatórios
2. WHEN credenciais AWS inválidas são fornecidas, THE Credentials_Manager SHALL exibir uma mensagem de erro identificando o campo inválido e o motivo da rejeição, e bloquear o início da sincronização
3. THE Credentials_Manager SHALL armazenar as credenciais utilizando o gerenciador de credenciais nativo do sistema operacional (Keychain no macOS, Credential Manager no Windows, Secret Service no Linux)
4. WHEN as credenciais expiram ou são revogadas, THE Credentials_Manager SHALL exibir uma notificação indicando que as credenciais são inválidas e pausar todas as operações de sincronização até que novas credenciais válidas sejam fornecidas
5. WHERE suporte a AWS IAM Roles estiver habilitado nas configurações, THE Credentials_Manager SHALL aceitar autenticação via perfil de instância ou role sem necessidade de Access Key ID e Secret Access Key
6. WHEN a sincronização é iniciada, THE Credentials_Manager SHALL verificar o acesso ao bucket S3 configurado realizando uma chamada de teste à API S3 antes de iniciar a sincronização
7. IF a verificação de acesso ao bucket S3 falhar, THEN THE Credentials_Manager SHALL exibir uma mensagem de erro descritiva indicando o motivo da falha e bloquear o início da sincronização
8. WHERE autenticação via STS (Security Token Service) for utilizada, THE Credentials_Manager SHALL aceitar Access Key ID, Secret Access Key e Session Token como parâmetros de autenticação

---

### Requisito 5: Configuração da Aplicação

**User Story:** Como usuário, quero configurar qual bucket S3 e qual pasta local serão sincronizados, para que eu possa personalizar o drive de acordo com minhas necessidades.

#### Critérios de Aceitação

1. THE S3_Drive SHALL permitir ao usuário especificar o nome do S3_Bucket a ser sincronizado
2. THE S3_Drive SHALL permitir ao usuário especificar o caminho absoluto da Pasta_Local no computador
3. WHERE um prefixo S3 for especificado nas configurações, THE S3_Drive SHALL sincronizar apenas os objetos cujo caminho começa com o prefixo configurado
4. IF uma configuração inválida de bucket for fornecida, THEN THE S3_Drive SHALL exibir uma mensagem de erro indicando o nome do campo inválido e o motivo da rejeição, sem iniciar a sincronização
5. THE Config_Parser SHALL ler e escrever as configurações do aplicativo em um arquivo no formato JSON com codificação UTF-8
6. WHEN o Config_Parser serializa e desserializa um arquivo de configuração, THE Config_Parser SHALL preservar os valores de todos os campos sem alteração
7. IF o caminho configurado para a Pasta_Local não existir ou não for acessível, THEN THE S3_Drive SHALL exibir uma mensagem de erro descritiva e não iniciar a sincronização
8. WHEN o aplicativo é iniciado pela primeira vez sem um arquivo de configuração existente, THE Config_Parser SHALL criar um arquivo de configuração vazio com os campos obrigatórios não preenchidos
9. IF o arquivo de configuração contiver JSON malformado ou dados não-UTF-8, THEN THE Config_Parser SHALL exibir uma mensagem de erro descritiva e não iniciar a sincronização

---

### Requisito 6: Resolução de Conflitos

**User Story:** Como usuário, quero que conflitos de arquivos sejam tratados de forma transparente, para que eu não perca dados quando o mesmo arquivo é modificado em dois lugares simultaneamente.

#### Critérios de Aceitação

1. WHEN um arquivo é modificado na Pasta_Local e no S3_Bucket desde a última sincronização bem-sucedida, THE Conflict_Resolver SHALL detectar o conflito antes de sobrescrever qualquer versão
2. WHEN um conflito é detectado, THE Conflict_Resolver SHALL preservar ambas as versões: a versão local mantém o nome original e a versão remota é salva com o sufixo de conflito no formato `[nome_base](conflito-AAAA-MM-DD-HH-MM-SS).[extensão]` na Pasta_Local
3. WHEN um conflito é detectado, THE Conflict_Resolver SHALL notificar o usuário com o nome do arquivo, data de modificação de cada versão no formato `AAAA-MM-DD HH:MM:SS UTC` e tamanho de cada versão em bytes
4. IF o timestamp de última modificação local diferir do timestamp da última sincronização bem-sucedida E o ETag do objeto S3 diferir do ETag registrado na última sincronização, THEN THE Conflict_Resolver SHALL classificar a situação como conflito
5. IF a cópia de conflito não puder ser salva na Pasta_Local, THEN THE Conflict_Resolver SHALL preservar a versão local, cancelar a sincronização do arquivo afetado e registrar a falha com o nome do arquivo e o motivo do erro

---

### Requisito 7: Indicadores de Status de Sincronização

**User Story:** Como usuário, quero visualizar o status de sincronização dos arquivos, para que eu saiba quando os arquivos estão sincronizados, em processo de sincronização ou com erro.

#### Critérios de Aceitação

1. THE Status_Monitor SHALL atribuir a cada arquivo um dos seguintes estados: `pendente` ao ser enfileirado, `sincronizando` ao iniciar a transferência, `sincronizado` ao concluir com sucesso, ou `erro` ao falhar
2. THE Status_Monitor SHALL exibir um ícone na área de notificação do sistema operacional indicando o estado geral de sincronização do drive, com precedência: `erro` > `sincronizando`/`pendente` > `sincronizado`
3. IF todos os arquivos estiverem com estado `sincronizado`, THEN THE Status_Monitor SHALL exibir o ícone de bandeja no estado "em dia"
4. WHILE arquivos estão com estado `sincronizando` ou `pendente`, THE Status_Monitor SHALL exibir um ícone animado de sincronização na área de notificação
5. WHEN um erro de sincronização ocorre, THE Status_Monitor SHALL exibir o ícone de erro na área de notificação
6. WHEN um erro de sincronização ocorre, THE Status_Monitor SHALL registrar os detalhes do erro em arquivo de log contendo: nome do arquivo, tipo de erro e timestamp com precisão de segundos

---

### Requisito 8: Suporte a Operações Offline

**User Story:** Como usuário, quero continuar trabalhando com meus arquivos quando não houver conexão com a internet, para que minha produtividade não seja afetada por problemas de conectividade.

#### Critérios de Aceitação

1. WHILE não há conexão com a internet disponível, THE Sync_Engine SHALL permitir leitura e escrita de arquivos na Pasta_Local retornando sucesso ao solicitante sem exibir mensagens de erro de conectividade
2. WHILE não há conexão com a internet disponível, THE Queue_Manager SHALL enfileirar todas as operações de sincronização pendentes em fila persistente em disco até o limite de 10.000 operações
3. WHEN a conexão com a internet é restaurada, THE Sync_Engine SHALL processar automaticamente todas as operações enfileiradas pelo Queue_Manager em ordem cronológica em até 30 segundos após a detecção da reconexão
4. IF uma operação offline enfileirada conflita com uma mudança detectada no S3_Bucket após a reconexão, THEN THE Conflict_Resolver SHALL aplicar as regras de resolução de conflitos definidas no Requisito 6
5. WHEN o Queue_Manager atinge o limite de 10.000 operações, THE Queue_Manager SHALL rejeitar novas operações de sincronização e notificar o usuário que o limite da fila foi atingido
6. IF uma operação enfileirada falha durante o processamento pós-reconexão, THEN THE Sync_Engine SHALL registrar a falha com o nome do arquivo e o motivo do erro, remover a operação da fila e continuar processando as operações restantes

---

### Requisito 9: Sincronização Seletiva

**User Story:** Como usuário, quero escolher quais pastas e arquivos sincronizar, para que eu possa economizar espaço em disco e largura de banda sincronizando apenas o que é necessário.

#### Critérios de Aceitação

1. THE S3_Drive SHALL permitir ao usuário selecionar pastas específicas do S3_Bucket para inclusão na sincronização, incluindo automaticamente todas as subpastas das pastas selecionadas
2. THE S3_Drive SHALL permitir ao usuário definir padrões de exclusão por nome de arquivo utilizando glob patterns (ex: `*.tmp`, `*.log`, `.DS_Store`)
3. IF um arquivo na Pasta_Local corresponder a um padrão de exclusão configurado, THEN THE Sync_Engine SHALL ignorar o arquivo sem enviá-lo ao S3_Bucket
4. IF um objeto no S3_Bucket corresponder a um padrão de exclusão configurado, THEN THE Sync_Engine SHALL ignorar o objeto sem baixá-lo para a Pasta_Local
5. WHEN o usuário modifica as regras de sincronização seletiva, THE S3_Drive SHALL aplicar as novas regras na próxima operação de sincronização iniciada, sem necessidade de reinicialização do aplicativo
6. IF um padrão de exclusão com sintaxe glob inválida for fornecido, THEN THE S3_Drive SHALL exibir uma mensagem de erro indicando o padrão inválido e rejeitar a alteração, preservando as regras existentes
7. WHEN um arquivo já existente na Pasta_Local passa a corresponder a um padrão de exclusão após uma alteração nas regras, THE Sync_Engine SHALL preservar o arquivo local sem excluí-lo automaticamente
8. IF um arquivo na Pasta_Local corresponder a um padrão de exclusão configurado, THEN THE Sync_Engine SHALL omitir o arquivo de todas as operações futuras de sincronização

---

### Requisito 10: Notificações ao Usuário

**User Story:** Como usuário, quero receber notificações sobre eventos importantes de sincronização, para que eu esteja ciente de erros ou conclusão de operações relevantes.

#### Critérios de Aceitação

1. WHEN a primeira sincronização completa dos arquivos do bucket é concluída com sucesso, THE Notification_Service SHALL exibir uma notificação ao usuário informando o número de arquivos sincronizados, visível por no mínimo 5 segundos
2. WHEN um erro de sincronização ocorre após 3 tentativas consecutivas com falha, THE Notification_Service SHALL exibir uma notificação persistente com o nome do arquivo e a descrição do erro, que permanece visível até que o usuário a dispense manualmente
3. WHEN um conflito de arquivo é detectado, THE Notification_Service SHALL exibir uma notificação persistente com o nome do arquivo em conflito e as opções: manter versão local, manter versão remota ou manter ambas as versões, que permanece visível até que a resolução seja concluída com sucesso. Em caso de falha, SHALL preservar o conflito, informar o erro e permitir nova tentativa; durante a resolução, SHALL desabilitar as opções desse conflito
4. THE S3_Drive SHALL permitir ao usuário desativar nas configurações as notificações de conclusão de sincronização e de conflito de arquivo, sem desativar as notificações de erro
5. WHEN o usuário solicita exclusão individual ou em lote, THE S3_Drive SHALL solicitar confirmação antes de executar qualquer exclusão, exibindo os arquivos, a quantidade, a pasta local, o bucket S3 e a consequência da remoção nos dois destinos. Cancelar SHALL preservar os arquivos; durante a execução, SHALL impedir confirmação duplicada
6. WHEN iniciar/parar a sincronização, abrir a pasta local ou carregar/limpar logs falhar, THE S3_Drive SHALL exibir uma mensagem útil e permitir nova tentativa, preservando os registros existentes em caso de falha de atualização
7. THE S3_Drive SHALL distinguir configuração ausente, estado indisponível, conexão em andamento, parada em andamento, motor parado, ausência de rede, erro e sincronização atualizada. O indicador de atividade SHALL consultar o estado real do motor, sem inferir atividade apenas dos estados dos arquivos ou da configuração salva
8. WHEN as credenciais forem inválidas ou expiradas, THE S3_Drive SHALL manter o alerta com acesso às configurações até uma inicialização de sincronização bem-sucedida

---

### Requisito 11: Configuração e navegação desktop

**User Story:** Como usuário, quero configurar a sincronização com orientação e controlar quando minhas alterações são aplicadas.

#### Critérios de Aceitação

1. WHEN a aplicação inicia com bucket e pasta local previamente configurados, THE S3_Drive SHALL abrir a tela de sincronização; caso contrário, SHALL abrir a configuração inicial. Uma falha ao ler a configuração SHALL permitir nova tentativa sem substituir a configuração salva.
2. THE S3_Drive SHALL organizar a configuração inicial nas etapas Conexão, Pasta e direção e Revisão e início, preservando os valores ao voltar. SHALL validar os campos antes de avançar e mostrar os destinos e a direção antes de iniciar.
3. THE S3_Drive SHALL recolher ajustes avançados e indicar alterações não salvas. Notificações e demais preferências SHALL ser aplicadas apenas por uma ação explícita de salvar; Salvar e iniciar SHALL salvar antes de iniciar. Uma falha SHALL preservar as edições pendentes.
4. THE S3_Drive SHALL preservar o rascunho ao alternar entre telas e permitir descartar alterações explicitamente. Credenciais digitadas SHALL permanecer apenas na memória da tela até seu salvamento no gerenciador seguro e SHALL ser limpas dos campos após salvamento bem-sucedido.
5. WHEN um perfil JSON é importado, THE S3_Drive SHALL carregar uma configuração validada para revisão sem persistir ou aplicar automaticamente. Exportação e teste de conexão SHALL usar a configuração salva, exigindo que alterações pendentes sejam salvas ou descartadas primeiro.
6. THE S3_Drive SHALL fazer o filtro Ativos mostrar todos os arquivos pendentes ou em sincronização, correspondendo ao contador apresentado.

### Requisito 12 — Publicação de site estático

1. A tela Sites permite escolher uma pasta local ou navegar pelas pastas do bucket sincronizado. A pasta escolhida precisa de `index.html` na raiz. Antes de publicar, o usuário revisa os arquivos, pode retirar itens e vê o tamanho total, o destino, o aviso de acesso público e a possibilidade de custos na AWS.
2. **Criar outro site** cria um bucket privado dedicado e uma distribuição HTTPS no CloudFront. O nome do bucket é editável e sugerido a partir da pasta. A interface permite escolher outro nome em caso de conflito e copiar o link de cada site publicado.
3. **Republicar** ativa a nova versão no endereço do site selecionado. **Restaurar versão** reativa uma versão registrada no mesmo endereço após confirmação. Alterações na origem entram no ar mediante uma nova publicação.
4. A tela exibe progresso e falhas da AWS. Uma falha durante o envio preserva a versão anterior. Conexões com endpoint S3 personalizado ficam indisponíveis para publicação.

### Requisito 13 — Sincronização de múltiplos buckets

1. O usuário pode adicionar buckets existentes com a conexão AWS configurada e escolher região, pasta local e prefixo para cada vínculo.
2. Os vínculos podem sincronizar simultaneamente. Cada vínculo adicional tem controles de início e pausa e mostra sua atividade e revisões de exclusão. O estado habilitado persiste entre sessões.
3. O cadastro rejeita buckets duplicados e pastas locais sobrepostas. A atualização preserva a configuração e o estado do bucket principal.
4. O usuário pode criar um bucket privado no Amazon S3 informando nome e região e configurar sua sincronização depois.

### Requisito 14 — Atualização do aplicativo

1. O aplicativo consulta o canal estável ao abrir e avisa quando encontra uma versão mais recente para o instalador em uso.
2. Após a confirmação do usuário, o aplicativo verifica a assinatura e a versão do pacote, encerra a sincronização dos buckets e instala a atualização.
3. Depois de reiniciar, o aplicativo mantém os dados de sincronização e retoma os vínculos habilitados.
