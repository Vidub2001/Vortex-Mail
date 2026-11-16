//! Adresses techniques des services en ligne O365 / Outlook.com (connexion
//! `OAuth2`, courrier, agenda et contacts). Regroupées ici : ce sont les seuls
//! noms de ces services dont le code a besoin pour fonctionner.

/// Serveur de connexion `OAuth2`.
pub(crate) const LOGIN_HOST: &str = "login.microsoftonline.com";
/// Comptes personnels (Outlook.com).
pub(crate) const TENANT: &str = "consumers";

/// Courrier : IMAP et SMTP par jeton `OAuth2`.
pub(crate) const MAIL_SCOPE: &str = "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access";
/// À la connexion seulement : `openid email` donne l'adresse réellement connectée.
pub(crate) const MAIL_SIGN_IN_SCOPE: &str = "https://outlook.office.com/IMAP.AccessAsUser.All \
    https://outlook.office.com/SMTP.Send offline_access openid email profile";

/// Agenda et contacts : API REST du service.
pub(crate) const PIM_API: &str = "https://graph.microsoft.com/v1.0";
pub(crate) const PIM_SCOPE: &str =
    "https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.ReadWrite offline_access";
pub(crate) const PIM_SIGN_IN_SCOPE: &str = "https://graph.microsoft.com/Calendars.ReadWrite \
    https://graph.microsoft.com/Contacts.ReadWrite https://graph.microsoft.com/Mail.Send offline_access openid email";
/// Autorisations demandées dès la connexion du courrier : agenda, contacts, et
/// envoi par l'API (boîtes dont le service a coupé l'envoi SMTP).
pub(crate) const PIM_CONSENT: &str = "https://graph.microsoft.com/Calendars.ReadWrite \
    https://graph.microsoft.com/Contacts.ReadWrite https://graph.microsoft.com/Mail.Send";
/// Envoi d'un message par l'API, quand le service refuse l'envoi SMTP de la
/// boîte (comptes récents : code 5.7.139).
pub(crate) const SEND_SCOPE: &str = "https://graph.microsoft.com/Mail.Send offline_access";
/// En-têtes de préférence de l'API : heures en UTC, corps en texte.
pub(crate) const PIM_PREFER_UTC: &str = "outlook.timezone=\"UTC\"";
pub(crate) const PIM_PREFER_TEXT: &str = "outlook.body-content-type=\"text\"";
