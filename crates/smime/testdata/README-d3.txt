Fichiers de test du lot D2/D3 de Vortex Mail, faits avec OpenSSL 3 :
- bob.key.der (PKCS #8) et bob.cert.der : RSA 2048, bob@example.org, auto-signé, 100 ans ;
- content.txt : le contenu chiffré ;
- enveloped-aes256.der (RSA PKCS #1 v1.5, AES-256-CBC), enveloped-oaep.der (RSAES-OAEP SHA-256, AES-128-CBC),
  enveloped-3des-oaep-sha1.der (OAEP SHA-1 par défaut, 3DES), enveloped-ber.der (BER, longueurs indéfinies),
  enveloped-other.der (chiffré pour un autre certificat), authenveloped-gcm.der (AES-256-GCM, lu depuis le lot G2b),
  enveloped.eml (message complet, base64).
Lot G2b (ECDH et AES-GCM), OpenSSL 3.0.2 :
- carol.key.der et carol.cert.der : P-256, carol@example.org ; dave.key.der et dave.cert.der : P-384, dave@example.org ;
  auto-signés, 100 ans, keyAgreement et digitalSignature ;
- enveloped-ec-p256.der (dérivation X9.63 SHA-1 par défaut, AES-256-CBC), enveloped-ec-p256-sha256.der
  (-keyopt ecdh_kdf_md:sha256, AES-128-CBC), authenveloped-ec-gcm.der (SHA-256, AES-256-GCM), enveloped-ec-p384.der
  (Dave, SHA-384, AES-192-CBC), enveloped-ec-p384-sha512.der (Dave, -keyopt ecdh_kdf_md:sha512, AES-256-CBC) ; tous chiffrent content.txt ;
- authenveloped-gcm.der (Bob, RSA, AES-256-GCM) se lit depuis le lot G2b.
Aucune de ces clés ne sert ailleurs.
