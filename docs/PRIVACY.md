# WolfMail Privacy Policy

> Draft for IntelligentWolf Ltd to approve and host at a page on `intelligentwolf.com` (Google requires the policy on
> the same verified domain as the app's homepage, linked from that homepage and from the OAuth consent screen).
> Every statement below about what the app does is taken from WolfMail's source; the cited file is noted in
> `docs/VERIFICATION.md` → "Where each statement comes from". Statements about IntelligentWolf's servers are commitments
> IntelligentWolf Ltd makes and must be true before this is published.

**Last updated:** 29 October 2026

WolfMail is a desktop email, calendar and contacts app made by IntelligentWolf Ltd ("we"). This policy explains what
WolfMail does with your data, including data it receives from Google and Microsoft when you sign in with them.

## The short version

- Your mail, calendars and contacts are downloaded **to your computer** and stored there. WolfMail has no server of
  its own that receives your mail.
- WolfMail talks directly to your mail provider (Google, Microsoft, or your IMAP/SMTP server).
- Nothing is sent to IntelligentWolf unless you choose to connect WolfMail to an IntelligentWolf account. Even then,
  only the senders' email addresses are sent automatically. The content of a message is sent only when you ask for
  an IntelligentWolf feature on that message.
- We do not sell your data, do not use it for advertising, and do not use it to train AI models.

## What WolfMail stores on your computer

- **Mail, folders, calendars and contacts** from the accounts you add, in a local database in WolfMail's application
  data folder. That lets you read and search offline.
- **Passwords and sign-in tokens** (the OAuth refresh tokens Google and Microsoft give WolfMail) in your operating
  system's credential store: Secret Service on Linux, Keychain on macOS, or Credential Manager on Windows. If your
  computer has no usable credential store, they go in an encrypted vault file protected by a password you choose.
  They are never written to the mail database or to logs.
- **Your settings.**

Removing a mail account in WolfMail deletes its local mail and its stored sign-in. Removing a calendar and contacts
account does the same for its calendars, contacts and sign-in. Uninstalling WolfMail and deleting
its application data folder removes everything.

## Data from Google (Gmail, Google Calendar, Google Contacts)

When you add a Google account, WolfMail asks Google for these permissions:

| Permission | What WolfMail uses it for |
|---|---|
| `gmail.modify` | Download your mail and labels so you can read and search it. Send mail and save drafts. Mark messages read or unread, star them, move them between labels and to Trash, and run the filters you set up in WolfMail. |
| `calendar.events`, `calendar.calendarlist.readonly`, `calendar.calendars` (only if you turn on calendars) | Show your list of Google calendars and their events. Create, change and delete events you edit in WolfMail, send your replies to invitations, and create a new calendar when you ask. |
| `contacts.readonly`, `contacts.other.readonly` (only if you turn on contacts) | Show your Google contacts and "Other contacts" for address autocomplete and the contacts view. WolfMail never changes them. |
| `openid`, `email` | Know which Google account you signed in with. |

This Google data goes **only** between Google's servers and your computer. It is not sent to IntelligentWolf or to
anyone else, except as described under "IntelligentWolf features" when you choose to use them.

**WolfMail's use and transfer to any other app of information received from Google APIs will adhere to the
[Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including
the Limited Use requirements.** In particular:

- Google user data is used only to provide the email, calendar and contacts features you see in WolfMail.
- It is not transferred to others except to provide those features at your request, for security, to comply with
  the law, or as part of a merger or acquisition with your prior consent.
- It is not used or transferred to serve ads, is not sold, and is not used to determine creditworthiness or for
  lending.
- No person at IntelligentWolf reads it. The only exceptions are when you ask us to, for example in a support
  request, or when it is needed for security or the law.
- It is not used to develop, improve or train generalised or non-personalised AI or machine-learning models.

## Data from Microsoft (Outlook.com, Microsoft 365)

When you add a Microsoft account, WolfMail asks Microsoft for Mail.ReadWrite, Mail.Send, User.Read, offline_access,
openid, profile and email. If you turn on calendars and contacts, it also asks for Calendars.ReadWrite and
Contacts.ReadWrite. They are used the same way as the Google permissions above: to read, organise and send your
mail, and to show and edit your calendars and contacts. The data goes only between Microsoft's servers and your
computer, under the same commitments as for Google data.

## IntelligentWolf features (optional)

You can connect WolfMail to an IntelligentWolf account in Settings → IntelligentWolf. WolfMail works fully without
it. When connected:

- **Automatically:** for each message visible in your list, one email address is sent to IntelligentWolf: the
  sender's, or the first recipient's on mail you sent. That lets WolfMail highlight people who are already leads or
  contacts in your IntelligentWolf workspace. No names, subjects or message content are sent.
- **Only when you choose**, and only the one message you choose:
  - **AI actions** (summarise, draft a reply, triage, extract contact details). The message's subject, sender,
    recipients, date and text go to IntelligentWolf, which processes them with Anthropic's Claude AI and returns the
    result.
  - **Ask** (natural-language search). Only the question you type is sent, never your mail.
  - **Log to CRM, Update contact, Make task.** The message details shown in the dialog are saved to your
    IntelligentWolf workspace, where people in your workspace with access can see them.
  - **Chat with the assistant.** Your chat message is sent, plus the open email only if it is attached. An attached
    email shows as a chip you can remove.
- What IntelligentWolf does with this data is covered by IntelligentWolf's privacy policy. IntelligentWolf follows
  the same Limited Use commitments above for anything that came from Google: it does not sell it, does not use it
  for ads, and does not use it to train AI models.
- To disconnect, use Settings → IntelligentWolf → Disconnect, or remove the device on IntelligentWolf's WolfMail
  page. The connection stops working immediately.

## Other connections WolfMail makes

- **Setting up an account:** to find your provider's server settings, WolfMail may look up your email domain (the
  part after the @, not your full address) in Mozilla's public ISP database (`autoconfig.thunderbird.net`) and on
  your domain's own autoconfig server, and may check its DNS records.
- **Updates:** WolfMail checks GitHub (`github.com/intelligentwolf/wolfmail`) for new versions. That request
  carries no personal data.
- **Remote images** in emails are blocked until you allow them, and known tracking pixels are removed, so senders
  can't see when you open their mail.
- WolfMail has **no analytics, telemetry or crash reporting**.

## Revoking access

You can remove WolfMail's access at any time:

- Google: <https://myaccount.google.com/permissions>
- Microsoft personal accounts: <https://account.live.com/consent/Manage>
- Microsoft work or school accounts: <https://myapps.microsoft.com>

You can also remove the account in WolfMail.

## Children

WolfMail is not directed at children under 13.

## Changes

If this policy changes, we will update the date at the top. If a change affects how data from Google or Microsoft
is used, the new version will be published before that change takes effect.

## Contact

IntelligentWolf Ltd: paul@wolf.uk.com
