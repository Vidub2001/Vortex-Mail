# Política de seguridad

[English](SECURITY.md) · [Français](SECURITY.fr.md) · **Español**

Vortex Mail lee cada día mensajes escritos por desconocidos: los problemas de seguridad se toman en serio. Gracias por comunicarlos en privado.

## Versiones mantenidas

Solo la última versión publicada recibe correcciones de seguridad. Comprueba que el problema sigue existiendo en ella antes de comunicarlo.

## Comunicar una vulnerabilidad

**Nunca en una incidencia pública.** Usa uno de estos canales privados:

1. **Preferiblemente:** la comunicación privada de GitHub, «[Report a vulnerability](https://github.com/Vidub2001/Vortex-Mail/security/advisories/new)» en la pestaña *Security* de este repositorio.
2. Si no: el [formulario de contacto](https://vortexmail.fr/es/contact.html) del sitio, pidiendo un canal privado.

Indica, por favor:

- la versión de Vortex Mail y tu sistema (distribución o versión de Windows);
- los pasos para reproducirlo, y lo que ocurre frente a lo que debería ocurrir;
- si el problema viene de un mensaje, el propio mensaje como archivo `.eml` (se añadirá al corpus de no regresión una vez corregido);
- lo que podría obtener un atacante, según tú.

## Alcance

Lo que protege Vortex Mail, y frente a quién, se describe en el [modelo de amenazas](docs/MODELE-MENACE.es.md). Un comportamiento que lo contradice es una vulnerabilidad.

Los límites que el modelo de amenazas ya declara (sección «Fuera del alcance y límites conocidos») no son vulnerabilidades, por ejemplo: un programa malicioso que se ejecuta con la cuenta del usuario, un remitente que escribe desde un dominio parecido, o la dirección IP que conoce quien aloja una imagen cuando el usuario decide mostrar las imágenes.

## Qué esperar

Vortex Mail lo mantiene una sola persona. Los compromisos son modestos, pero se cumplen:

- acuse de recibo de tu comunicación en **7 días**;
- una corrección, o al menos una actualización del estado, en **30 días**;
- divulgación coordinada: la vulnerabilidad se hace pública una vez publicada la versión corregida, y como muy tarde **90 días** después de tu comunicación, salvo acuerdo distinto.

Si lo deseas, se te mencionará en las notas de la versión corregida. No hay recompensa económica.
