//! Expéditeur authentifié (lot G1) : résultats SPF, DKIM et DMARC notés par le
//! serveur de réception dans l'en-tête `Authentication-Results` (RFC 8601).
//!
//! Seul le premier de ces en-têtes compte : le serveur de réception ajoute le
//! sien en tête du message, au-dessus de ceux que l'expéditeur aurait pu écrire
//! lui-même. Un échec l'emporte toujours sur une réussite : un en-tête forgé
//! ne peut que provoquer un avertissement, jamais en cacher un. Rien n'est
//! affiché quand l'expéditeur est authentifié ou que le serveur ne dit rien.

use mail_core::compose;

/// Le domaine de l'expéditeur refuse ce message (DMARC en échec).
pub(crate) const DMARC_FAIL: &str = "dmarc";
/// Sans politique DMARC : serveur d'envoi non autorisé (SPF en échec) et aucune signature DKIM valide.
pub(crate) const SPF_FAIL: &str = "spf";

/// Ce qu'il faut signaler de l'expéditeur de `raw` : vide si rien,
/// [`DMARC_FAIL`] ou [`SPF_FAIL`].
pub(crate) fn verdict(raw: &[u8]) -> &'static str {
    compose::header_value(raw, "Authentication-Results").map_or("", |value| judge(&value))
}

/// Verdict d'une valeur d'en-tête `Authentication-Results`.
fn judge(value: &str) -> &'static str {
    let text = without_comments(value);
    let (mut dmarc, mut spf, mut dkim_pass) = (None, None, false);
    // authserv-id, puis « ; méthode=résultat propriétés… » (RFC 8601 §2.2) ;
    // certains serveurs omettent l'authserv-id.
    for part in text.split(';') {
        let Some((method, rest)) = part.trim().split_once('=') else { continue };
        let method = method.split('/').next().unwrap_or_default().trim().to_ascii_lowercase();
        let result = rest.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
        match method.as_str() {
            "dmarc" if dmarc.is_none() || result == "fail" => dmarc = Some(result),
            "spf" if spf.is_none() || result == "fail" => spf = Some(result),
            "dkim" => dkim_pass |= result == "pass",
            _ => {}
        }
    }
    match (dmarc.as_deref(), spf.as_deref()) {
        (Some("fail"), _) => DMARC_FAIL,
        (None | Some("none"), Some("fail")) if !dkim_pass => SPF_FAIL,
        _ => "",
    }
}

/// Texte sans ses commentaires `( … )`, imbriqués compris (RFC 5322 §3.2.2).
fn without_comments(value: &str) -> String {
    let mut depth = 0usize;
    let mut out = String::with_capacity(value.len());
    let mut escaped = false;
    for c in value.chars() {
        match c {
            _ if escaped => escaped = false,
            '\\' if depth > 0 => escaped = true,
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(results: &[&str]) -> Vec<u8> {
        let mut raw = String::from("Return-Path: <x@banque.example>\r\n");
        for r in results {
            raw.push_str("Authentication-Results: ");
            raw.push_str(r);
            raw.push_str("\r\n");
        }
        raw.push_str("From: Banque <service@banque.example>\r\nSubject: Urgent\r\n\r\nCorps\r\n");
        raw.into_bytes()
    }

    #[test]
    fn expediteur_usurpe_signale() {
        // Gmail : DMARC en échec (politique « reject »).
        let gmail = "mx.google.com;\r\n       dkim=fail header.i=@banque.example;\r\n       spf=fail (google.com: domain of x@pirate.example does not designate 1.2.3.4) smtp.mailfrom=x@pirate.example;\r\n       dmarc=fail (p=REJECT sp=REJECT dis=NONE) header.from=banque.example";
        assert_eq!(verdict(&mail(&[gmail])), DMARC_FAIL, "en-tête sur plusieurs lignes");
        // Outlook.com : sans authserv-id.
        let outlook = "spf=fail (sender IP is 1.2.3.4) smtp.mailfrom=pirate.example; dkim=none (message not signed) header.d=none;dmarc=fail action=quarantine header.from=banque.example;compauth=fail reason=000";
        assert_eq!(verdict(&mail(&[outlook])), DMARC_FAIL);
        // Domaine sans politique DMARC : SPF en échec et aucune signature valide.
        assert_eq!(verdict(&mail(&["mx.mailo.com; spf=fail smtp.mailfrom=banque.example; dkim=none; dmarc=none"])), SPF_FAIL);
    }

    #[test]
    fn expediteur_authentifie_ou_inconnu() {
        let ok = "mx.google.com; dkim=pass header.i=@banque.example; spf=pass smtp.mailfrom=banque.example; dmarc=pass (p=REJECT) header.from=banque.example";
        assert_eq!(verdict(&mail(&[ok])), "");
        // Liste de diffusion : SPF en échec, signature DKIM valide.
        assert_eq!(verdict(&mail(&["mx.mailo.com; spf=fail smtp.mailfrom=liste.example; dkim=pass header.d=liste.example"])), "");
        // SPF « softfail » : trop fréquent à tort (renvois) pour être signalé seul.
        assert_eq!(verdict(&mail(&["mx.mailo.com; spf=softfail smtp.mailfrom=banque.example"])), "");
        // Aucun en-tête : rien à dire.
        assert_eq!(verdict(&mail(&[])), "");
        assert_eq!(verdict(&mail(&["mx.mailo.com; none"])), "");
    }

    #[test]
    fn seul_le_premier_en_tete_compte() {
        // L'expéditeur a écrit son propre en-tête, sous celui du serveur de réception.
        let server = "mx.mailo.com; dmarc=fail header.from=banque.example";
        let forged = "mx.mailo.com; dkim=pass; spf=pass; dmarc=pass header.from=banque.example";
        assert_eq!(verdict(&mail(&[server, forged])), DMARC_FAIL, "l'en-tête forgé ne cache pas l'échec");
        // Un échec et une réussite pour la même méthode : l'échec l'emporte.
        assert_eq!(verdict(&mail(&["mx.mailo.com; dmarc=pass; dmarc=fail header.from=banque.example"])), DMARC_FAIL);
    }

    #[test]
    fn commentaires_et_casse() {
        assert_eq!(without_comments("a (b (c) d) e (f\\) g) h"), "a  e  h");
        assert_eq!(verdict(&mail(&["MX.Example; DMARC=Fail (p=reject; « ; » dans un commentaire) header.from=x"])), DMARC_FAIL);
    }
}
