// Fenêtres secondaires : un message seul, ou la rédaction d'un message.
import {
  FluentProvider,
  Spinner,
  Toast,
  ToastBody,
  ToastTitle,
  Toaster,
  useId,
  useToastController,
} from "@fluentui/react-components";
import { useCallback, useEffect, useState } from "react";
import { api, runningInTauri } from "./api";
import { t } from "./i18n";
import { answerInvitation, applyInvitation } from "./invitation";
import { ActionBar } from "./components/ActionBar";
import { ConfirmDialog } from "./components/Dialogs";
import { Compose } from "./components/Compose";
import { ReadingPane } from "./components/ReadingPane";
import { draftInit, newDraft, replyDraft, type ComposeInit } from "./compose";
import { darkTheme, lightTheme, useThemeMode } from "./theme";
import type { Account, AttachmentInfo, AttachmentInput, Draft, Folder, FolderRole, Message } from "./types";
import { closeThisWindow, notifyMain, openComposeWindow, setThisWindowTitle, takeComposeInit } from "./windows";

export default function Popup({ params }: { params: URLSearchParams }) {
  const theme = useThemeMode();
  return (
    <FluentProvider theme={theme.dark ? darkTheme : lightTheme} className={theme.dark ? "provider dark" : "provider light"}>
      <div className="popup-root">
        {params.get("view") === "compose" ? (
          <ComposeWindow composeKey={params.get("key") ?? ""} draftId={params.has("draft") ? Number(params.get("draft")) : null} />
        ) : (
          <MessageWindow id={Number(params.get("id"))} dark={theme.dark} />
        )}
      </div>
    </FluentProvider>
  );
}

function useNotify() {
  const toasterId = useId("toaster");
  const { dispatchToast } = useToastController(toasterId);
  const notify = useCallback(
    (title: string, body?: string, intent: "success" | "error" | "info" = "success") =>
      dispatchToast(
        <Toast>
          <ToastTitle>{title}</ToastTitle>
          {body && <ToastBody>{body}</ToastBody>}
        </Toast>,
        // Erreurs affichées plus longtemps : elles demandent souvent une action.
        { intent, timeout: intent === "error" ? 8000 : 6000 },
      ),
    [dispatchToast],
  );
  return { toaster: <Toaster toasterId={toasterId} position="bottom-end" />, notify };
}

/** Un message dans sa propre fenêtre, avec la même barre d'actions. */
function MessageWindow({ id, dark }: { id: number; dark: boolean }) {
  const { toaster, notify } = useNotify();
  const [message, setMessage] = useState<Message | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.getMessage(id).then((m) => {
      setMessage(m);
      setThisWindowTitle(m.subject || t("(sans objet)"));
    }, (e) => setError(String(e)));
    api.listAccounts().then(setAccounts).catch(() => {});
    api.listFolders().then((l) => setFolders(l.folders)).catch(() => {});
  }, [id]);

  const fail = (e: unknown) => notify(t("Erreur"), String(e), "error");

  /** Déplacement ou suppression : la fenêtre n'a plus d'objet, on la ferme. */
  const moved = async (job: Promise<unknown>) => {
    try {
      await job;
      await notifyMain({ type: "moved", ids: [id] });
      await closeThisWindow();
    } catch (e) {
      fail(e);
    }
  };

  const toggle = async (kind: "seen" | "flagged") => {
    if (!message) return;
    const on = !message.flags[kind];
    try {
      if (kind === "seen") await api.setSeen([id], on);
      else await api.setFlagged([id], on);
      setMessage({ ...message, flags: { ...message.flags, [kind]: on } });
      notifyMain({ type: "flags", ids: [id] });
    } catch (e) {
      fail(e);
    }
  };

  const toRole = (role: FolderRole) => moved(api.moveToRole([id], role));

  /** Supprimer : un brouillon demande confirmation, comme dans la fenêtre principale. */
  const [confirmDelete, setConfirmDelete] = useState(false);
  const isDraft = !!message && folders.find((f) => f.id === message.folderId)?.role === "drafts";
  const remove = () => (isDraft ? setConfirmDelete(true) : toRole("trash"));

  const openAttachment = async (m: Message, a: AttachmentInfo) => {
    await api.openAttachment(m.id, a.index).catch((e) => notify(t("Ouverture impossible"), String(e), "error"));
  };
  const saveAttachments = async (m: Message, list: AttachmentInfo[]) => {
    for (const a of list) {
      try {
        notify(t("Pièce jointe enregistrée"), await api.saveAttachment(m.id, a.index));
      } catch (e) {
        notify(t("Échec : {name}", { name: a.name }), String(e), "error");
      }
    }
  };
  const showImages = async (m: Message) => {
    try {
      setMessage(await api.showImages(m.id));
    } catch (e) {
      notify(t("Images non affichées"), String(e), "error");
    }
  };
  const checkSignature = async (m: Message, quiet: boolean) => {
    try {
      const updated = await api.checkSignature(m.id);
      setMessage((cur) => (cur && cur.id === updated.id ? updated : cur));
    } catch (e) {
      if (!quiet) notify(t("Signature non vérifiée"), String(e), "error");
    }
  };

  if (error) return <div className="popup-error">{error}</div>;
  if (!message) return <Spinner className="popup-loading" size="small" />;

  return (
    <>
      <ReadingPane
        message={message}
        loading={false}
        selectionCount={1}
        dark={dark}
        toolbar={
          <ActionBar
            accounts={accounts}
            folders={folders}
            canReply
            selectionCount={1}
            allSeen={message.flags.seen}
            allFlagged={message.flags.flagged}
            onReply={(mode) => openComposeWindow(replyDraft(mode, message, accounts))}
            onArchive={() => toRole("archive")}
            onDelete={remove}
            onMove={(folderId) => moved(api.moveMessages([id], folderId))}
            onToggleSeen={() => toggle("seen")}
            onToggleFlag={() => toggle("flagged")}
          />
        }
        onOpenAttachment={openAttachment}
        onSaveAttachments={saveAttachments}
        onShowImages={showImages}
        onCheckSignature={checkSignature}
        onAddInvitation={async (m, a) => {
          try {
            const { done, title } = await applyInvitation(m, a);
            notify(done, title);
          } catch (e) {
            notify(t("Agenda non modifié"), String(e), "error");
          }
        }}
        onAnswerInvitation={async (m, a, answer, send, asAccount) => {
          try {
            const { done, title } = await answerInvitation(m, a, answer, send, asAccount);
            notify(done, title);
          } catch (e) {
            notify(t("Réponse non envoyée"), String(e), "error");
          }
        }}
      />
      {confirmDelete && (
        <ConfirmDialog
          title={t("Supprimer le brouillon ?")}
          message={t("Ce brouillon sera déplacé dans « Éléments supprimés ».")}
          confirm={t("Supprimer")}
          onConfirm={async () => {
            await toRole("trash");
          }}
          onClose={() => setConfirmDelete(false)}
        />
      )}
      {toaster}
    </>
  );
}

/**
 * Rédaction dans sa propre fenêtre ; fermée après l'envoi.
 * `draftId` : brouillon enregistré à reprendre (double-clic dans « Brouillons »).
 */
function ComposeWindow({ composeKey, draftId }: { composeKey: string; draftId: number | null }) {
  const { toaster } = useNotify();
  const [init, setInit] = useState<ComposeInit | null>(() => (draftId === null ? takeComposeInit(composeKey) : null));
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.listAccounts().then(setAccounts).catch(() => setAccounts([]));
    if (draftId !== null) {
      api.openDraft(draftId).then((d) => {
        setInit(draftInit(d));
        setThisWindowTitle(d.subject || t("Brouillon"));
      }, (e) => setError(String(e)));
    }
  }, [draftId]);

  if (error) return <div className="popup-error">{t("Brouillon illisible : {error}", { error })}</div>;
  if (!accounts || (draftId !== null && !init)) return <Spinner className="popup-loading" size="small" />;
  const draft = init ?? newDraft(accounts[0]?.id ?? "");

  const send = async (
    accountId: string,
    d: Draft,
    answeredId: number | null,
    attachments: AttachmentInput[],
    savedDraft: number | null,
  ) => {
    // Envoi en arrière-plan (lot E1) : la fenêtre se ferme dès que le backend a
    // le message ; la fenêtre principale annonce le résultat, ou rouvre le
    // message si l'envoi échoue.
    await api.sendMessage(accountId, d, answeredId, attachments, savedDraft);
    // Aperçu navigateur : la maquette annonce elle-même « en cours » puis le
    // résultat, dans cet ordre (la fenêtre se ferme juste après).
    if (runningInTauri) await notifyMain({ type: "sending", accountId });
    await closeThisWindow();
  };

  const discard = async (savedDraft: number | null) => {
    if (savedDraft !== null) {
      await api.discardDraft(savedDraft).catch(() => {});
      await notifyMain({ type: "draft" });
    }
    await closeThisWindow();
  };

  return (
    <>
      <Compose key={draft.key} init={draft} accounts={accounts} onSend={send} onDiscard={discard} />
      {toaster}
    </>
  );
}
