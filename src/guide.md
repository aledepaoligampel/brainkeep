Brainkeep es una memoria compartida entre asistentes y personas. No guarda archivos ni conversaciones completas: guarda recuerdos breves (decisiones, restricciones, descubrimientos, avances, ideas en evaluación) asociados a un proyecto.

Proyecto: es un nombre, no una carpeta, y pertenece a quien lo crea: tus proyectos son solo tuyos aunque otra persona use el mismo nombre. Para que el asistente use siempre el mismo nombre, conviene escribirlo en el CLAUDE.md o AGENTS.md de la carpeta de trabajo, por ejemplo: "Proyecto de Brainkeep: demo. Consultá y guardá siempre en ese proyecto."

Compartir: el dueño comparte un proyecto con el email de otra persona invitada con `share` (rol read o write) y lo retira con `unshare`. Quien lo recibe lo usa como dueño/proyecto, por ejemplo ana/demo. `projects` lista tus proyectos y los compartidos con vos.

Guardar: el asistente guarda solo lo relevante, sin que haga falta pedirlo, aunque no está garantizado en cada turno. También se le puede pedir explícitamente: "Guardá en Brainkeep que elegimos X porque Y". Para cambiar una decisión, decirlo: se crea una revisión nueva y se conserva el historial.

Consultar: preguntar con el nombre del proyecto, por ejemplo "¿Qué decidimos en equipo/demo sobre la base de datos?" o "¿Qué falta hacer en equipo/demo?".

Permisos: cada credencial puede leer y escribir solo ciertos nombres de proyecto (ver read y write en esta respuesta). `*` significa todos; `prefijo/*` incluye los que empiezan con `prefijo/`. Escribir permite leer. Fuera de esos permisos la respuesta es "project not accessible", exista o no. Un proyecto ajeno no compartido se comporta como inexistente. Cada revisión registra el usuario que la guardó (principal).

No guardar contraseñas, tokens ni datos privados: Brainkeep rechaza formatos conocidos de secretos, pero no todos. Lo que un asistente recupera también lo procesa su proveedor.
