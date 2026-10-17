import { api } from '../data/api'
import { confirm, toast, toastError } from '../ui/feedback'
import { t } from '../i18n'

/** Restart or shut down the device after a confirmation; the top bar and System share it. */
export async function powerAction(action: 'reboot' | 'shutdown') {
  const ok = await confirm({
    title: action === 'reboot' ? t('Reboot the device?') : t('Shut down the device?'),
    body:
      action === 'reboot'
        ? t('All connections will drop for about 10-30 seconds.')
        : t('The device powers off. Use the physical power button to turn it back on.'),
    confirmLabel: action === 'reboot' ? t('Reboot') : t('Shut down'),
    danger: true,
  })
  if (!ok) return
  try {
    if (action === 'reboot') {
      await api.reboot()
      toast(t('Reboot command sent'))
    } else {
      await api.shutdown()
      toast(t('Shutdown command sent'))
    }
  } catch (e) {
    toastError(e, action === 'reboot' ? t('Failed to reboot device') : t('Failed to shutdown device'))
  }
}
