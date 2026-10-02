// A screen not built yet: its title and a line of text. Later parts of the
// web UI replace each with the real view.
export default function Placeholder({ title, text, detail }: { title: string; text: string; detail?: string }) {
  return (
    <div className="welcome">
      <h1>{title}</h1>
      {detail !== undefined && (
        <p className="mono">
          <bdi>{detail}</bdi>
        </p>
      )}
      <p>{text}</p>
    </div>
  )
}
