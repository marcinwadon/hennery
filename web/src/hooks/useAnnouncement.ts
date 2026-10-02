// What a session view's one live region says: the answer book's last
// announcement, a question that opened without taking the focus.
import { useSyncExternalStore } from 'react'
import type { AnswerBook } from '../store/useAnswer'

export function useAnnouncement(book: AnswerBook): string {
  return useSyncExternalStore(book.subscribe, () => book.announcement)
}
