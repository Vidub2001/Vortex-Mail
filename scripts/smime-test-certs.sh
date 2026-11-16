#!/usr/bin/env bash
# Certificats S/MIME de test (lot D1 de docs/plan-lots.md) : une autorité
# « Autorité de test Vortex Mail », un certificat de signature et de
# chiffrement pour une adresse, et des messages signés ou chiffrés à déposer
# dans une boîte IMAP.
#
#   scripts/smime-test-certs.sh adresse@exemple.fr [dossier]
#
# Dans le dossier (par défaut `smime-test`, ignoré par git) :
#   ca.pem               l'autorité, à copier dans le dossier des autorités de
#                        Vortex Mail pour que la signature soit « vérifiée » :
#                        ~/.config/fr.vortexmail.client/smime/autorites (Linux),
#                        %APPDATA%\fr.vortexmail.client\smime\autorites (Windows),
#                        data/config/smime/autorites (mode portable) ;
#                        Vortex Mail lit ce dossier une fois, au démarrage.
#   signataire.p12       certificat et clé du signataire (mot de passe : vortex),
#                        pour le lot D2 ou pour un autre logiciel de messagerie ;
#   signe-detache.eml    message signé (multipart/signed) ;
#   signe-opaque.eml     message signé « opaque » (application/pkcs7-mime) ;
#   signe-modifie.eml    message signé puis modifié : « modifié après sa signature » ;
#   chiffre.eml          message chiffré pour ce certificat (AES-256) ;
#   signe-chiffre.eml    message signé puis chiffré.
#
# Dépôt d'un message dans la boîte de réception, sans l'envoyer (IMAP APPEND) :
#   curl --url "imaps://imap.exemple.fr/INBOX" --user "adresse@exemple.fr" -T smime-test/signe-detache.eml
#
# Clés sans mot de passe dans le dossier : pour des essais seulement.
set -euo pipefail

address=${1:?usage : scripts/smime-test-certs.sh adresse@exemple.fr [dossier]}
dir=${2:-smime-test}
case "$address" in
  *@*.*) ;;
  *) echo "Adresse invalide : $address" >&2; exit 1 ;;
esac
mkdir -p "$dir"
cd "$dir"

# Autorité : RSA 3072, valable un an.
openssl req -x509 -newkey rsa:3072 -sha256 -days 365 -nodes -keyout ca.key -out ca.pem \
  -subj "/CN=Autorité de test Vortex Mail" -utf8 \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null

# Signataire : l'adresse dans le sujet et dans subjectAltName, usage
# « messagerie sécurisée » (RFC 8550).
openssl req -newkey rsa:3072 -sha256 -nodes -keyout signataire.key -out signataire.csr \
  -subj "/CN=Test Vortex Mail/emailAddress=$address" 2>/dev/null
cat > signataire.ext <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=emailProtection
subjectAltName=email:$address
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid
EOF
openssl x509 -req -in signataire.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 365 -sha256 \
  -extfile signataire.ext -out signataire.pem 2>/dev/null
openssl pkcs12 -export -inkey signataire.key -in signataire.pem -certfile ca.pem \
  -name "Test Vortex Mail" -out signataire.p12 -passout pass:vortex
rm -f signataire.csr signataire.ext ca.srl

# Messages : texte UTF-8, signé en SHA-256, lignes en CRLF ; objet en RFC 2047.
printf 'Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\nBonjour,\r\n\r\nCe message est signé avec le certificat de test de Vortex Mail.\r\n\r\nÀ bientôt.\r\n' > contenu.txt
sign() {
  local subject=$1 out=$2
  shift 2
  {
    printf 'Date: %s\r\nMessage-ID: <%s.%s@vortexmail.test>\r\n' "$(LC_ALL=C date -R)" "$(date +%s%N)" "$out"
    openssl cms -sign "$@" -in contenu.txt -binary -signer signataire.pem -inkey signataire.key -md sha256 -crlfeol \
      -from "$address" -to "$address" -subject "$subject"
  } > "$out"
}
sign "=?UTF-8?Q?Essai_S/MIME_:_signature_d=C3=A9tach=C3=A9e?=" signe-detache.eml
sign "=?UTF-8?Q?Essai_S/MIME_:_signature_opaque?=" signe-opaque.eml -nodetach
sed -e 's/^Bonjour,/Bonjour, ceci a été ajouté ensuite,/' -e 's/signature_d=C3=A9tach=C3=A9e/message_modifi=C3=A9_apr=C3=A8s_sa_signature/' \
  signe-detache.eml > signe-modifie.eml

# Messages chiffrés pour le certificat du signataire (lot D3a) : AES-256, puis
# signé et chiffré (la signature, à l'intérieur, se lit une fois déchiffré).
encrypt() {
  local subject=$1 out=$2 input=$3
  {
    printf 'Date: %s\r\nMessage-ID: <%s.%s@vortexmail.test>\r\n' "$(LC_ALL=C date -R)" "$(date +%s%N)" "$out"
    openssl cms -encrypt -in "$input" -binary -aes256 -crlfeol -from "$address" -to "$address" -subject "$subject" signataire.pem
  } > "$out"
}
encrypt "=?UTF-8?Q?Essai_S/MIME_:_message_chiffr=C3=A9?=" chiffre.eml contenu.txt
openssl cms -sign -in contenu.txt -binary -signer signataire.pem -inkey signataire.key -md sha256 -crlfeol > signe.tmp
encrypt "=?UTF-8?Q?Essai_S/MIME_:_sign=C3=A9_et_chiffr=C3=A9?=" signe-chiffre.eml signe.tmp
rm -f signe.tmp
rm -f contenu.txt

echo "Fait dans $(pwd) :"
ls -1
