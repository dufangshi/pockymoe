# Account and sign-in

## Your account key {#account-key}

Your password unlocks your account key in the browser. The relay keeps only an encrypted copy; it never sees your password and cannot unlock it for you. A browser you signed in to receives a long-lived certificate, so you do not need to enter the password every time you open it.

The Account page shows your **safety code** and **account ID** for comparing with devices or people you share with.

## Unlock this browser {#unlock}

This browser has no browser key for you yet: you are signing in here for the first time, or the key was deleted. Enter your account password. Accounts that sign in with Google or GitHub enter the unlock password they chose.

## Sign in again {#sign-in-again}

- **This browser's saved key was cleared**: the site data was cleared while you stayed signed in. Choose **Sign in again** and sign in once with your password.
- **Finishing setup needs a fresh sign-in**: you signed in more than 10 minutes ago, so saving the account key needs a new verification.

## Recovery code {#recovery-code}

The recovery code has 8 groups of 4 characters and is shown once when you create or secure your account. It is the only way to recover your account key if you forget the password, so keep it offline. The relay administrator cannot recover it for you either.

You can generate a new recovery code on the Account page; the old one stops working right away.

## Forgot password {#forgot-password}

On the sign-in page choose **Forgot password?** → **Recovery code**, and enter your username, recovery code and a new password. Your bound devices and signed-in browsers stay.

## Admin password {#admin-reset}

Without a recovery code, ask the relay administrator for a temporary password, then sign in with **Forgot password?** → **Admin password**. Afterwards:

- With your recovery code: enter it and a new password; all devices and data stay.
- Without it: you can only **Start over** with a new account key.

## Start over {#start-over}

Starting over creates a new account key. Devices bound before refuse it, so run `pockymoe bind --reset` on each device and protect it again. People you shared devices with need to confirm your new identity too.

## Why the password is asked for {#password-prompts}

After signing in, everyday use does not ask for the password. It is asked for when:

- You protect or repair a device, remove a device or browser, or change your password. These are signed with your account key; after unlocking, you are not asked again for 10 minutes.
- This browser's key was cleared, or this browser was removed.
- Your sign-in expired (14 days).
