import {
  Button,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Input,
  MessageBar,
  MessageBarBody,
  Select,
  Spinner,
  ToggleButton,
} from "@fluentui/react-components";
import { formatTime, loadGeneral } from "../general";
import {
  Attach20Regular,
  Delete20Regular,
  Dismiss16Regular,
  LockClosed20Regular,
  Save20Regular,
  Send20Regular,
  ShieldCheckmark20Regular,
} from "@fluentui/react-icons";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, runningInTauri } from "../api";
import { t } from "../i18n";
import { parseAddresses, type ComposeAttachment, type ComposeInit } from "../compose";
import { fileSize } from "../format";
import type { Account, AttachmentInput, Draft, SigningAccount } from "../types";
import { FileTypeIcon } from "./FileTypeIcon";
import { FormatToolbar } from "./FormatToolbar";
import { setHtml } from "../safe-html";
import { AddressInput } from "./AddressInput";
import { notifyMain, onCloseRequested } from "../windows";
import { clearBackup, writeBackup } from "../drafts";
import { attachmentMention, loadSettings, ownText, signatureContent } from "../settings";

/** Limite d'envoi, identique à celle du backend. */
const MAX_TOTAL = 25 * 1024 * 1024;

let fileCounter = 0;

/** Format de rédaction par défaut (Paramètres › Général). */
function loadPlain(): boolean {
  return loadGeneral().composeFormat === "text";
}

const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** HTML → texte, tel que le navigateur le rend (sauts de ligne compris). */
/**
 * Texte brut : chaque ligne d'un bloc cité est précédée de « > » (« >> » pour
 * une citation dans une citation). `root` doit être dans le document.
 */
function prefixQuotes(root: HTMLElement) {
  [...root.querySelectorAll("blockquote")].reverse().forEach((q) => {
    const lines = q.innerText.replace(/\n+$/, "").split("\n");
    const block = document.createElement("div");
    block.innerText = lines.map((l) => (l.startsWith(">") ? `>${l}` : `> ${l}`)).join("\n");
    q.replaceWith(block);
  });
}

function htmlToPlain(html: string): string {
  const div = document.createElement("div");
  setHtml(div, html);
  div.style.position = "fixed";
  div.style.left = "-10000px";
  document.body.appendChild(div);
  prefixQuotes(div);
  const text = div.innerText;
  div.remove();
  return text;
}

/** Texte brut → HTML équivalent (une ligne par bloc). */
function textToHtml(text: string): string {
  return text
    .split("\n")
    .map((line) => `<div>${line ? escapeHtml(line) : "<br>"}</div>`)
    .join("");
}

/** Contenu d'un fichier en base64 (sans l'en-tête `data:…;base64,`). */
function readBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result);
      resolve(url.slice(url.indexOf(",") + 1));
    };
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

/** Délai sans modification avant l'enregistrement automatique du brouillon. */
const AUTOSAVE_MS = 5000;

const isNetworkError = (message: string) =>
  /» introuvable|” not found|” no encontrado|connexion à .* impossible|could not connect|no se puede conectar|connexion fermée|connection closed|conexión cerrada|unreachable|timed out|délai|network|error de red|résolution|failed to lookup|erreur réseau/i.test(
    message,
  );

const time = () => formatTime(new Date());

interface Props {
  init: ComposeInit;
  accounts: Account[];
  /** Rejette avec le message d'erreur si l'envoi échoue (le brouillon reste ouvert). */
  onSend: (
    accountId: string,
    draft: Draft,
    answeredId: number | null,
    attachments: AttachmentInput[],
    draftId: number | null,
  ) => Promise<void>;
  /** « Abandonner » : `draftId` est le brouillon enregistré à supprimer, s'il existe. */
  onDiscard: (draftId: number | null) => void;
}

const TITLES: Record<ComposeInit["mode"], string> = {
  new: t("Nouveau message"),
  reply: t("Répondre"),
  replyAll: t("Répondre à tous"),
  forward: t("Transférer"),
  draft: t("Brouillon"),
};

export function Compose({ init, accounts, onSend, onDiscard }: Props) {
  const [accountId, setAccountId] = useState(init.accountId || accounts[0]?.id || "");
  const [to, setTo] = useState(init.to);
  const [cc, setCc] = useState(init.cc);
  const [bcc, setBcc] = useState(init.bcc ?? "");
  const [showCc, setShowCc] = useState(init.cc !== "");
  const [showBcc, setShowBcc] = useState((init.bcc ?? "") !== "");
  const [subject, setSubject] = useState(init.subject);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(init.error ?? null);
  const [files, setFiles] = useState<ComposeAttachment[]>(init.attachments);
  const [dropping, setDropping] = useState(false);
  const [plain, setPlain] = useState(() => init.plainText ?? loadPlain());
  /** Comptes qui ont un certificat de signature S/MIME (lot D2). */
  const [signers, setSigners] = useState<SigningAccount[]>([]);
  /** « Signer » enfoncé : le message part signé en S/MIME. */
  const [sign, setSign] = useState(false);
  /** « Chiffrer » enfoncé (lot D3b), et destinataires sans certificat connu. */
  const [encrypt, setEncrypt] = useState(init.encrypt ?? false);
  const [unencryptable, setUnencryptable] = useState<string[]>([]);
  /** Destinataires dont le certificat vient d'une autorité inconnue (lot D4). */
  const [unverified, setUnverified] = useState<string[]>([]);
  const canSign = signers.some((s) => s.accountId === accountId);
  /** État de l'enregistrement automatique, affiché en haut à droite. */
  const [status, setStatus] = useState<string | null>(null);
  /** Nombre de modifications : relance le minuteur d'enregistrement automatique. */
  const [changes, setChanges] = useState(0);
  /**
   * Empreinte du contenu tel qu'il a été ouvert, puis tel qu'il a été
   * enregistré : on n'enregistre que si le contenu en diffère. Ouvrir une
   * réponse, un transfert ou un nouveau message puis le fermer sans rien
   * changer ne crée donc pas de brouillon.
   */
  const reference = useRef<string | null>(null);
  /** L'utilisateur a agi dans la fenêtre (clavier, souris, collage, dépôt). */
  const userActed = useRef(false);
  const draftId = useRef<number | null>(init.draftId ?? null);
  const saving = useRef<Promise<void> | null>(null);
  /** Envoyé ou abandonné : plus rien à enregistrer. */
  const closed = useRef(false);
  const touched = useCallback(() => setChanges((n) => n + 1), []);
  /** Clé de la copie de secours, propre à cette fenêtre de rédaction. */
  const backupKey = useRef(`${Date.now()}-${Math.random().toString(36).slice(2)}`);
  /** Clé d'envoi de cette fenêtre (relecture complète n° 9). */
  const sendKey = useRef(Array.from(crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, "0")).join(""));
  const editor = useRef<HTMLDivElement>(null);
  const toInput = useRef<HTMLInputElement>(null);
  const filePicker = useRef<HTMLInputElement>(null);
  const total = files.reduce((n, f) => n + f.size, 0);

  const addFiles = async (list: FileList | File[]) => {
    const picked = [...list].filter((f) => f.size > 0 || f.type !== "");
    if (picked.length === 0) return;
    try {
      const added = await Promise.all(
        picked.map(async (f) => ({
          key: `f${++fileCounter}`,
          name: f.name,
          contentType: f.type || "application/octet-stream",
          size: f.size,
          data: await readBase64(f),
        })),
      );
      setFiles((current) => [...current, ...added]);
      setError(null);
    } catch (e) {
      setError(t("Lecture du fichier impossible : {error}", { error: String(e) }));
    }
  };

  const hasFiles = (e: React.DragEvent) => [...e.dataTransfer.types].includes("Files");

  // Contenu initial posé une seule fois : l'éditeur gère ensuite son propre DOM.
  useEffect(() => {
    if (editor.current) {
      if (plain) editor.current.innerText = htmlToPlain(init.html);
      else setHtml(editor.current, init.html);
    }
    if (init.mode === "new" || init.mode === "forward") toInput.current?.focus();
    else placeCaretAtStart();
  }, [init]);

  const placeCaretAtStart = () => {
    const el = editor.current;
    if (!el) return;
    el.focus();
    const range = document.createRange();
    range.setStart(el, 0);
    range.collapse(true);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
  };

  /** Bascule texte ↔ HTML : le contenu est converti (la mise en forme est perdue en texte). */
  const switchFormat = (toPlain: boolean) => {
    const el = editor.current;
    if (toPlain === plain || !el) return;
    if (toPlain) {
      prefixQuotes(el);
      el.innerText = el.innerText;
    }
    else setHtml(el, textToHtml(el.innerText));
    setPlain(toPlain);
    // Curseur en fin de texte.
    el.focus();
    const range = document.createRange();
    range.selectNodeContents(el);
    range.collapse(false);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
  };

  /** Le message tel qu'il est saisi. */
  const collect = (): { draft: Draft; attachments: AttachmentInput[] } | null => {
    const account = accounts.find((a) => a.id === accountId);
    if (!account) return null;
    const draft: Draft = {
      fromName: "",
      fromEmail: account.email,
      to: parseAddresses(to),
      cc: showCc ? parseAddresses(cc) : [],
      bcc: showBcc ? parseAddresses(bcc) : [],
      subject: subject.trim(),
      html: plain ? textToHtml(editor.current?.innerText ?? "") : (editor.current?.innerHTML ?? ""),
      plainText: plain,
      inReplyTo: init.inReplyTo,
      references: init.references,
      sendKey: sendKey.current,
      sign: canSign && sign,
      encrypt: canSign && encrypt,
    };
    const attachments: AttachmentInput[] = files.map(({ key, size, ...input }) => {
      void key;
      void size;
      return input;
    });
    return { draft, attachments };
  };

  // Toujours la version à jour, pour le minuteur et la fermeture de la fenêtre.
  const collectRef = useRef(collect);
  collectRef.current = collect;

  /** Empreinte du contenu saisi (pièces jointes : sans leur contenu, seulement sa taille). */
  const fingerprintOf = (built: ReturnType<typeof collect>): string =>
    built ? JSON.stringify([built.draft, built.attachments.map((a) => ({ ...a, data: a.data?.length }))]) : "";
  const fingerprintRef = useRef(() => fingerprintOf(collectRef.current()));
  fingerprintRef.current = () => fingerprintOf(collectRef.current());
  const fingerprint = () => fingerprintRef.current();
  /** Le contenu diffère de celui ouvert ou du dernier enregistré. */
  const isDirty = () => reference.current !== null && fingerprint() !== reference.current;
  const acted = () => {
    userActed.current = true;
  };
  const accountRef = useRef(accountId);
  accountRef.current = accountId;

  /**
   * Enregistre le brouillon dans « Brouillons » (et une copie de secours sur
   * cet ordinateur tant que le serveur ne l'a pas). Renvoie `true` si tout est enregistré.
   */
  const saveDraft = useCallback(async (force = false): Promise<boolean> => {
    if (saving.current) await saving.current;
    if (closed.current) return true;
    // Rien de neuf : inutile de redéposer (sauf demande explicite d'un brouillon jamais enregistré).
    if (!isDirty() && (!force || draftId.current !== null)) {
      if (force) setStatus(t("Brouillon enregistré à {time}", { time: time() }));
      return true;
    }
    const built = collectRef.current();
    if (!built) return false;
    const marker = fingerprintOf(built);
    const backup = { accountId: accountRef.current, draft: built.draft, draftId: draftId.current };
    // Message à chiffrer (lot D4) : pas de copie de secours en clair sur cet ordinateur ;
    // le brouillon part chiffré dans « Brouillons ».
    if (built.draft.encrypt) clearBackup(backupKey.current);
    else writeBackup(backupKey.current, backup);
    setStatus(t("Enregistrement du brouillon…"));
    const job = api.saveDraft(accountRef.current, built.draft, built.attachments, draftId.current);
    saving.current = job.then(
      () => {},
      () => {},
    );
    try {
      const saved = await job;
      draftId.current = saved.id;
      reference.current = marker;
      if (!closed.current) setStatus(t("Brouillon enregistré à {time}", { time: time() }));
      clearBackup(backupKey.current);
      notifyMain({ type: "draft" });
      return true;
    } catch (e) {
      const message = String(e);
      setStatus(
        isNetworkError(message)
          ? t("Hors connexion : brouillon gardé sur cet ordinateur")
          : t("Brouillon non enregistré : {message}", { message }),
      );
      return false;
    } finally {
      saving.current = null;
    }
  }, []);

  // Changement de compte d'envoi : la signature suit (si le message en contient une).
  // Certificats de signature : lus à l'ouverture ; « Signer » suit le réglage
  // du compte choisi.
  useEffect(() => {
    let alive = true;
    api
      .smimeSigners()
      .then((list) => alive && setSigners(list))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  useEffect(() => {
    const byDefault = signers.find((s) => s.accountId === accountId)?.signByDefault ?? false;
    // Envoi échoué (lot E1) : « Signer » reste tel que l'utilisateur l'avait mis.
    setSign(accountId === init.accountId && init.sign !== undefined ? init.sign : byDefault);
  }, [accountId, signers, init.accountId, init.sign]);
  // Chiffrer : les destinataires sans certificat sont nommés tout de suite.
  useEffect(() => {
    if (!encrypt || !canSign) {
      setUnencryptable([]);
      setUnverified([]);
      return;
    }
    let alive = true;
    const all = [...parseAddresses(to), ...(showCc ? parseAddresses(cc) : []), ...(showBcc ? parseAddresses(bcc) : [])];
    const timer = window.setTimeout(() => {
      api
        .smimeMissing(all)
        .then((check) => {
          if (!alive) return;
          setUnencryptable(check.missing);
          setUnverified(check.unverified);
        })
        .catch(() => {});
    }, 300);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [encrypt, canSign, to, cc, bcc, showCc, showBcc]);

  const previousAccount = useRef(accountId);
  useEffect(() => {
    if (previousAccount.current === accountId) return;
    previousAccount.current = accountId;
    const block = editor.current?.querySelector<HTMLElement>("[data-signature]");
    if (!block) return;
    const content = signatureContent(accountId);
    if (content) setHtml(block, content);
    else block.remove();
  }, [accountId]);

  // Tant que l'utilisateur n'a rien fait, les changements d'état viennent de
  // l'ouverture (contenu initial, liste des comptes chargée, double rendu de
  // React en développement) : ils définissent le contenu de référence.
  useEffect(() => {
    if (!userActed.current) {
      // Brouillon retrouvé : rien n'est encore sur le serveur, tout est « modifié ».
      reference.current = init.recovered ? "" : fingerprint();
      return;
    }
    touched();
  }, [accountId, to, cc, bcc, subject, files, plain, showCc, showBcc, touched]);

  // Brouillon retrouvé au démarrage : enregistré tout de suite, pour qu'il ait
  // sa propre copie de secours et sa place sur le serveur (relecture complète n° 33).
  useEffect(() => {
    if (!init.recovered) return;
    const timer = window.setTimeout(() => void saveDraft(), 1_000);
    return () => window.clearTimeout(timer);
  }, [init.recovered, saveDraft]);

  // Enregistrement automatique quelques secondes après la dernière modification.
  useEffect(() => {
    if (changes === 0) return;
    const timer = window.setTimeout(() => void saveDraft(), AUTOSAVE_MS);
    return () => window.clearTimeout(timer);
  }, [changes, saveDraft]);

  // Fermeture de la fenêtre : on enregistre d'abord ce qui ne l'est pas encore.
  useEffect(
    () =>
      onCloseRequested(async () => {
        if (closed.current || !isDirty()) return;
        await Promise.race([saveDraft(), new Promise((r) => window.setTimeout(r, 8000))]);
      }),
    [saveDraft],
  );

  /** « Abandonner » : confirmation s'il y a quelque chose à perdre. */
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const askDiscard = () => {
    const hasContent =
      draftId.current !== null ||
      isDirty() ||
      to.trim() !== "" ||
      subject.trim() !== "" ||
      files.length > 0;
    if (hasContent) setConfirmDiscard(true);
    else discard();
  };

  const discard = async () => {
    setConfirmDiscard(false);
    closed.current = true;
    clearBackup(backupKey.current);
    // Un enregistrement automatique en cours doit finir : le brouillon qu'il
    // crée ou remplace sur le serveur est supprimé lui aussi (relecture F12).
    if (saving.current) await saving.current;
    onDiscard(draftId.current);
  };

  /** Pièce jointe annoncée mais absente : mot trouvé (fenêtre d'avertissement ouverte). */
  const [missingAttachment, setMissingAttachment] = useState<string | null>(null);

  const send = async (checked = false) => {
    if (sending) return;
    const built = collect();
    if (!built) return setError(t("Choisissez le compte d'envoi."));
    const { draft, attachments } = built;
    if (draft.to.length + draft.cc.length + draft.bcc.length === 0) {
      toInput.current?.focus();
      return setError(t("Ajoutez au moins un destinataire."));
    }
    if (total > MAX_TOTAL) {
      return setError(
        t("Pièces jointes trop volumineuses : {size} (limite {limit}).", { size: fileSize(total), limit: fileSize(MAX_TOTAL) }),
      );
    }
    if (!checked && files.length === 0 && loadSettings().attachmentReminder && editor.current) {
      const word = attachmentMention(`${draft.subject}\n${ownText(editor.current, plain)}`);
      if (word) return setMissingAttachment(word);
    }
    setError(null);
    setSending(true);
    // Un enregistrement en cours doit finir : son identifiant sera supprimé après l'envoi.
    if (saving.current) await saving.current;
    closed.current = true;
    // Envoi en arrière-plan (lot E1) : la fenêtre se ferme tout de suite. La copie de
    // secours, à jour, reste jusqu'à ce que le backend ait envoyé le message (sauf
    // message à chiffrer : jamais de copie en clair, lot D4).
    // Aperçu navigateur : pas de backend pour la retirer après l'envoi, que la maquette
    // fait aussitôt (un échec rouvre la rédaction, qui écrit la sienne).
    if (!runningInTauri) clearBackup(backupKey.current);
    else if (!draft.encrypt) writeBackup(backupKey.current, { accountId, draft, draftId: draftId.current });
    try {
      await onSend(accountId, draft, init.answeredId, attachments, draftId.current);
    } catch (e) {
      closed.current = false;
      setError(String(e));
      setSending(false);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      send();
    }
    if (e.key.toLowerCase() === "s" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void saveDraft(true);
    }
  };

  return (
    <section
      className={dropping ? "pane reading compose dropping" : "pane reading compose"}
      aria-label={TITLES[init.mode]}
      onKeyDown={onKeyDown}
      onKeyDownCapture={acted}
      onPointerDownCapture={acted}
      onPasteCapture={acted}
      onDropCapture={acted}
      onDragOver={(e) => {
        if (!hasFiles(e) || sending) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
        setDropping(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropping(false);
      }}
      onDrop={(e) => {
        if (!hasFiles(e)) return;
        e.preventDefault();
        setDropping(false);
        addFiles(e.dataTransfer.files);
      }}
    >
      <div className="compose-actions">
        <Button
          appearance="primary"
          icon={sending ? <Spinner size="tiny" /> : <Send20Regular />}
          disabled={sending}
          onClick={() => send()}
          title={t("Envoyer (Ctrl+Entrée)")}
        >
          {t("Envoyer")}
        </Button>
        <Button
          appearance="subtle"
          icon={<Save20Regular />}
          disabled={sending}
          onClick={() => void saveDraft(true)}
          title={t("Enregistrer dans « Brouillons » (Ctrl+S)")}
        >
          {t("Enregistrer")}
        </Button>
        <Button appearance="subtle" icon={<Delete20Regular />} disabled={sending} onClick={askDiscard}>
          {t("Abandonner")}
        </Button>
        {canSign && (
          <ToggleButton
            appearance="subtle"
            checked={sign}
            icon={<ShieldCheckmark20Regular />}
            disabled={sending}
            onClick={() => setSign((on) => !on)}
            title={t("Signer le message avec votre certificat S/MIME : le destinataire peut vérifier qu'il vient de vous et n'a pas été modifié.")}
          >
            {t("Signer")}
          </ToggleButton>
        )}
        {canSign && (
          <ToggleButton
            appearance="subtle"
            checked={encrypt}
            icon={<LockClosed20Regular />}
            disabled={sending}
            onClick={() => setEncrypt((on) => !on)}
            title={t("Chiffrer le message : seuls les destinataires pourront le lire (il faut avoir reçu de chacun un message signé).")}
          >
            {t("Chiffrer")}
          </ToggleButton>
        )}
        <input
          ref={filePicker}
          type="file"
          multiple
          hidden
          onChange={(e) => {
            if (e.target.files) addFiles(e.target.files);
            e.target.value = "";
          }}
        />
        <span className="compose-title muted" aria-live="polite">
          {status ?? TITLES[init.mode]}
        </span>
      </div>

      {error && (
        <MessageBar intent="error" className="compose-error">
          <MessageBarBody>{error}</MessageBarBody>
        </MessageBar>
      )}
      {unverified.length > 0 && unencryptable.length === 0 && (
        <MessageBar intent="warning" className="compose-error">
          <MessageBarBody>
            {t(
              "Certificat d'une autorité inconnue pour : {addresses}. Avant d'écrire des informations sensibles, vérifiez avec eux son empreinte (Paramètres › Courrier).",
              { addresses: unverified.join(", ") },
            )}
          </MessageBarBody>
        </MessageBar>
      )}
      {unencryptable.length > 0 && (
        <MessageBar intent="warning" className="compose-error">
          <MessageBarBody>
            {t("Impossible de chiffrer pour : {addresses}. Il faut avoir reçu de chacun un message signé, ou envoyer sans chiffrer.", {
              addresses: unencryptable.join(", "),
            })}
          </MessageBarBody>
        </MessageBar>
      )}

      <div className="compose-fields">
        <label className="compose-label" htmlFor="compose-from">{t("De")}</label>
        <Select id="compose-from" value={accountId} onChange={(_, d) => setAccountId(d.value)} disabled={sending}>
          {accounts.map((a) => (
            <option key={a.id} value={a.id}>
              {a.email} — {a.name}
            </option>
          ))}
        </Select>
        <span />

        <label className="compose-label" htmlFor="compose-to">{t("À")}</label>
        <AddressInput
          id="compose-to"
          ref={toInput}
          value={to}
          onChange={setTo}
          placeholder={t("Nom ou adresse : les contacts sont proposés")}
        />
        <span className="compose-ccbcc">
          {!showCc && (
            <Button size="small" appearance="transparent" onClick={() => setShowCc(true)}>
              {t("Cc")}
            </Button>
          )}
          {!showBcc && (
            <Button size="small" appearance="transparent" onClick={() => setShowBcc(true)}>
              {t("Cci")}
            </Button>
          )}
        </span>

        {showCc && (
          <>
            <label className="compose-label" htmlFor="compose-cc">{t("Cc")}</label>
            <AddressInput id="compose-cc" value={cc} onChange={setCc} />
            <span />
          </>
        )}
        {showBcc && (
          <>
            <label className="compose-label" htmlFor="compose-bcc">{t("Cci")}</label>
            <AddressInput id="compose-bcc" value={bcc} onChange={setBcc} />
            <span />
          </>
        )}

        <span />
        <Input
          appearance="underline"
          value={subject}
          onChange={(_, d) => setSubject(d.value)}
          placeholder={t("Ajouter un objet")}
          aria-label={t("Objet")}
          className="compose-subject"
        />
        <span />
      </div>

      {files.length > 0 && (
        <div className="attachment-list compose-attachments">
          {files.map((f) => (
            <div key={f.key} className="attachment-card" title={f.name}>
              <FileTypeIcon name={f.name} contentType={f.contentType} />
              <div className="attachment-text">
                <span className="attachment-name">{f.name}</span>
                <span className="attachment-size">{fileSize(f.size)}</span>
              </div>
              <Button
                size="small"
                appearance="subtle"
                icon={<Dismiss16Regular />}
                title={t("Retirer")}
                aria-label={t("Retirer {name}", { name: f.name })}
                disabled={sending}
                onClick={() => setFiles((list) => list.filter((x) => x.key !== f.key))}
              />
            </div>
          ))}
          {files.length > 1 && (
            <span className={total > MAX_TOTAL ? "attachment-total over" : "attachment-total"}>
              {t("{n} fichiers · {size}", { n: files.length, size: fileSize(total) })}
            </span>
          )}
        </div>
      )}

      <FormatToolbar
        editor={editor}
        disabled={sending}
        plain={plain}
        onPlainChange={switchFormat}
        onAttach={() => filePicker.current?.click()}
      />

      <div
        ref={editor}
        className={plain ? "compose-editor plain" : "compose-editor"}
        contentEditable={sending ? false : plain ? "plaintext-only" : true}
        suppressContentEditableWarning
        role="textbox"
        aria-multiline="true"
        aria-label={t("Corps du message")}
        onInput={touched}
      />
      {dropping && <div className="drop-hint">{t("Déposez les fichiers pour les joindre")}</div>}
      {missingAttachment && (
        <Dialog open onOpenChange={(_, d) => !d.open && setMissingAttachment(null)}>
          <DialogSurface className="small-dialog">
            <DialogBody>
              <DialogTitle>{t("Pièce jointe oubliée ?")}</DialogTitle>
              <DialogContent>
                <p className="dialog-text">
                  {t("Le message mentionne « {word} », mais aucun fichier n'est joint.", { word: missingAttachment })}
                </p>
              </DialogContent>
              <DialogActions>
                <Button
                  appearance="primary"
                  icon={<Attach20Regular />}
                  onClick={() => {
                    setMissingAttachment(null);
                    filePicker.current?.click();
                  }}
                >
                  {t("Joindre un fichier")}
                </Button>
                <Button
                  onClick={() => {
                    setMissingAttachment(null);
                    void send(true);
                  }}
                >
                  {t("Envoyer quand même")}
                </Button>
              </DialogActions>
            </DialogBody>
          </DialogSurface>
        </Dialog>
      )}
      {confirmDiscard && (
        <Dialog open onOpenChange={(_, d) => !d.open && setConfirmDiscard(false)}>
          <DialogSurface className="small-dialog">
            <DialogBody>
              <DialogTitle>{t("Supprimer le brouillon ?")}</DialogTitle>
              <DialogContent>
                <p className="dialog-text">
                  {draftId.current !== null
                    ? t("Ce message sera fermé et son brouillon supprimé du dossier « Brouillons ».")
                    : t("Ce message sera fermé sans être enregistré.")}
                </p>
              </DialogContent>
              <DialogActions>
                <Button appearance="primary" className="danger-button" onClick={discard}>
                  {t("Supprimer")}
                </Button>
                <Button onClick={() => setConfirmDiscard(false)}>{t("Annuler")}</Button>
              </DialogActions>
            </DialogBody>
          </DialogSurface>
        </Dialog>
      )}
    </section>
  );
}
