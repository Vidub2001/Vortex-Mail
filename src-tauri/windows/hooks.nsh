; Crochets de l'installateur Windows (NSIS), appelés par celui de Tauri
; (`bundle.windows.nsis.installerHooks`, lot M2).
;
; Les notifications de Vortex Mail portent l'identifiant de l'application
; (AppUserModelID `fr.vortexmail.client`, identifiant de tauri.conf.json).
; Windows en tire le nom et l'icône affichés dans le centre de notifications
; par le raccourci du menu Démarrer. Un poste passé par FluxMail garde un
; raccourci à l'ancien identifiant (`fr.fluxmail.client`) : une mise à jour ne
; change que sa cible, pas son identifiant. Les notifications y apparaissaient
; sous « fr.vortexmail.client », sans nom ni icône.
;
; L'identifiant est donc aussi déclaré dans le registre de l'utilisateur, comme
; Microsoft le décrit pour les applications non empaquetées : nom et icône des
; notifications, quel que soit le raccourci. L'icône `notifications.png` est
; posée dans le dossier d'installation par `tauri.windows.conf.json`.

!define VORTEX_AUMID_KEY "Software\Classes\AppUserModelId\fr.vortexmail.client"

!macro NSIS_HOOK_POSTINSTALL
  WriteRegExpandStr HKCU "${VORTEX_AUMID_KEY}" "DisplayName" "Vortex Mail"
  WriteRegExpandStr HKCU "${VORTEX_AUMID_KEY}" "IconUri" "$INSTDIR\notifications.png"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegKey HKCU "${VORTEX_AUMID_KEY}"
!macroend
