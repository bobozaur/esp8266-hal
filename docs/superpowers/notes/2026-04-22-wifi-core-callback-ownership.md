# WiFi Core Callback Ownership

## Rule

The ESP8266 target backend may use one backend-local global raw pointer slot to
bridge SDK callbacks into Rust. The pointee itself must live inside
caller-owned `WifiResources`.

## Model

- `WifiResources` owns `CallbackContext`
- the target backend installs `CallbackContext` into one `AtomicPtr<()>`
- installation is guarded by compare-and-exchange and returns a
  `CallbackRegistration` token
- the registration token is stored in the target backend
- dropping the registration token clears the global slot
- host-native tests never touch this slot because they use `FakeBackend`

## Soundness Conditions

- only one live target backend may own the callback slot
- the raw pointer always points to storage owned by the caller
- callback trampolines immediately recover `CallbackContext` and then delegate
  to safe broker and packet methods
- no event broker or packet ring may be global
