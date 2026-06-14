export default function Fatal({ message }: { message: string }) {
  return (
    <main className="center">
      <p className="error">Fatal: {message}</p>
    </main>
  );
}
