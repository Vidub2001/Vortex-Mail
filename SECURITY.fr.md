# Politique de sécurité

[English](SECURITY.md) · **Français** · [Español](SECURITY.es.md)

Vortex Mail lit chaque jour des messages écrits par des inconnus : les problèmes de sécurité sont pris au sérieux. Merci de les signaler en privé.

## Versions suivies

Seule la dernière version publiée reçoit les correctifs de sécurité. Vérifiez que le problème existe toujours dans celle-ci avant de le signaler.

## Signaler une faille

**Jamais dans un ticket public.** Utilisez l'un de ces canaux privés :

1. **De préférence :** le signalement privé de GitHub, « [Report a vulnerability](https://github.com/Vidub2001/Vortex-Mail/security/advisories/new) » dans l'onglet *Security* de ce dépôt.
2. Sinon : le [formulaire de contact](https://vortexmail.fr/fr/contact.html) du site, en demandant un canal privé.

Merci d'indiquer :

- la version de Vortex Mail et votre système (distribution ou version de Windows) ;
- les étapes pour reproduire, et ce qui se passe par rapport à ce qui devrait se passer ;
- si le problème vient d'un message, le message lui-même en fichier `.eml` (il rejoindra le corpus de non-régression une fois corrigé) ;
- ce qu'un attaquant pourrait obtenir, d'après vous.

## Périmètre

Ce que protège Vortex Mail, et contre qui, est décrit dans le [modèle de menace](docs/MODELE-MENACE.md). Un comportement qui le contredit est une faille.

Les limites que le modèle de menace déclare déjà (section « Hors périmètre et limites connues ») ne sont pas des failles, par exemple : un programme malveillant qui tourne sous le compte de l'utilisateur, un expéditeur qui écrit depuis un domaine sosie, ou l'adresse IP apprise par l'hébergeur d'une image une fois que l'utilisateur a choisi d'afficher les images.

## À quoi vous attendre

Vortex Mail est maintenu par une seule personne. Les engagements sont donc modestes, mais tenus :

- accusé de réception de votre signalement sous **7 jours** ;
- un correctif, ou au moins un point d'étape, sous **30 jours** ;
- publication coordonnée : la faille est rendue publique une fois la version corrigée publiée, et au plus tard **90 jours** après votre signalement, sauf accord contraire.

Si vous le souhaitez, vous serez remercié dans les notes de la version corrigée. Il n'y a pas de prime.
