# Backend Redesign Summary

## Strategic Shift: Pure KYC Engine & Proxy Backend

The verif-fyi-backend architecture is being fundamentally redesigned to improve security, simplify integration, and isolate responsibilities. Instead of the backend handling complex keypair device binding and acting as an OAuth2 Resource Server for external JWTs, we are splitting the architecture into two distinct layers:

1. **KYC Core Engine (verif-fyi-backend)**: A pure, stateless KYC state machine.
2. **KYC Proxy Backend**: An intermediary gateway that handles all authentication, session management, and secure URL generation.

### 1. The KYC Core Engine (Verif-fyi-backend)

The `verif-fyi-backend` will be stripped of all public key cryptography and user-facing authentication mechanisms.

- **No Keypairs**: The concepts of `device-id`, `pubkey-id`, and Keycloak keypair bindings are completely removed.
- **Pure State Machine**: The backend solely focuses on executing KYC flows, step transitions, and data persistence.
- **Server-to-Server Auth Only**: All endpoints (formerly `/bff/*`, `/staff/*`, and `/kc/*`) are now protected exclusively by **HMAC signatures** (or similar server-to-server authentication like mTLS).
- **No Direct Public Access**: The core backend is never exposed directly to the public internet or end-user clients.

### 2. The New KYC Proxy Backend

The KYC Proxy Backend is a new, lightweight service that sits between the client applications (Yew Portal, React Admin Manager) and the KYC Core Engine.

- **Gateway Role**: It receives all client requests, authenticates them, and forwards them to the Core Engine by signing them with the internal HMAC shared secret.
- **Client Authentication**: It handles the issuance and validation of secure HTTP-only cookies for both the portal users and the admin staff.
- **Secure URL Generation**: It manages the generation and validation of short-lived one-time codes (OTCs) to securely grant access to the portal without exposing sensitive tokens in URLs.

---

## Secure Portal Access Pattern (Avoiding Tokens in URLs)

To provide a seamless but highly secure redirect from a Use Case Server (or external application) to the Yew Portal, we employ a **One-Time Code (OTC) Exchange** pattern. This ensures no persistent access tokens are leaked in browser histories, referer headers, or local storage.

### The Exchange Flow:

1. **Initiation**:
   - The Use Case Server requests a new KYC session via the Proxy Backend (using HMAC auth).
   - The Proxy Backend calls the Core Engine to create the session and flows.
   - The Proxy Backend generates a **Short-Lived One-Time Code (OTC)** linked to this session, stores it server-side (e.g., in Redis) with a brief expiration (e.g., 5 minutes), and returns it to the Use Case Server.
2. **Redirection**:
   - The Use Case Server redirects the user's browser to the Yew Portal, appending the code: `https://portal.kyc.internal/?code=XYZ123`
3. **Code Exchange (Client-Side)**:
   - The Yew Portal reads `?code=XYZ123` from the URL.
   - The Portal immediately makes a `POST /proxy/auth/exchange { "code": "XYZ123" }` request to the Proxy Backend.
   - The Portal then removes the code from the browser's URL history using the History API (`history.replaceState`).
4. **Cookie Issuance (Server-Side)**:
   - The Proxy Backend validates the OTC, marks it as used/deletes it, and responds with a `Set-Cookie` header containing a secure, `HttpOnly`, `SameSite=Strict` session cookie.
5. **Authenticated Operation**:
   - All subsequent API calls from the Yew Portal to the Proxy Backend automatically include this secure cookie. The Proxy Backend validates the cookie and forwards the request to the Core Engine using HMAC.

### Why this is better:

- **No LocalStorage**: Tokens are never stored in `localStorage` or `sessionStorage`, mitigating XSS data exfiltration.
- **No URL Leakage**: The OTC is single-use and removed from the URL immediately, meaning if the URL is copied or logged, it is already invalid.

---

## Key Design Decisions

| Decision             | Rationale                                                                                           |
| :------------------- | :-------------------------------------------------------------------------------------------------- |
| **Remove Keypairs**  | Simplifies the architecture immensely. Device binding is out of scope for a pure KYC engine.        |
| **HMAC Core Auth**   | Ensures the core engine only accepts requests from trusted internal services (the proxy).           |
| **Introduce Proxy**  | Decouples authentication and session routing from the complex state machine logic.                  |
| **HttpOnly Cookies** | The most secure mechanism for browser-based SPAs, preventing XSS token theft.                       |
| **Single-Use OTC**   | Allows seamless cross-domain redirection without the risks associated with long-lived JWTs in URLs. |

## Conclusion

This redesign significantly reduces the attack surface and complexity of the `verif-fyi-backend`. By extracting user-facing authentication into a dedicated Proxy Backend and enforcing server-to-server HMAC authentication on the core engine, the system becomes a robust, highly secure, pure KYC state machine.
