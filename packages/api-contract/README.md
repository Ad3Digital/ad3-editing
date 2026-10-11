# @ad3-editing/api-contract

Vendored local da antiga `@diffusionstudio/api-contract@0.1.0` (MIT, ver
`LICENSE`), copiada byte a byte de `index.mjs` e `index.d.mts`.

Motivo: o pacote original vinha do registry npm sob o escopo antigo, o que
quebrava `npm install` após o rebranding total para `@ad3-editing/*` e
exigia rede/registro para algo que o operador proibiu usar (recursos
online/com tokens — ver `AGENTS.md` na raiz do repo).

Conteúdo: apenas schemas zod e tipos do AppRouter tRPC do backend em nuvem.
**Congelado**: não evoluir este pacote; a superfície online está depreciada
e a operação é 100% local.
